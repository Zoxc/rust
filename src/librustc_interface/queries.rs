use crate::interface::{Compiler, Result};
use crate::passes::{self, BoxedResolver, ExpansionResult, BoxedGlobalCtxt};

use rustc_incremental::DepGraphFuture;
use rustc_data_structures::sync::{Lrc, Lock};
use rustc::session::config::{Input, OutputFilenames, OutputType};
use rustc::session::Session;
use rustc::util::common::{time, ErrorReported};
use rustc::util::profiling::ProfileCategory;
use rustc::lint;
use rustc::hir;
    use rustc::hir::def_id::{LocalCrate, LOCAL_CRATE};
use rustc::ty;
use rustc::ty::steal::Steal;
use rustc::dep_graph::DepGraph;
use rustc_passes::hir_stats;
use rustc_plugin::registry::Registry;
use serialize::json;
use std::cell::{Ref, RefMut, RefCell};
use std::ops::Deref;
use std::sync::Arc;
use std::sync::mpsc;
use std::any::Any;
use std::mem;
use syntax::parse::{self, PResult};
use syntax::util::node_count::NodeCounter;
use syntax::{self, ast, attr, diagnostics, visit};
use syntax_pos::hygiene;

/// Represent the result of a query.
/// This result can be stolen with the `take` method and returned with the `give` method.
pub struct Query<T> {
    result: RefCell<Option<Result<T>>>,
}

impl<T> Query<T> {
    fn compute<F: FnOnce() -> Result<T>>(&self, f: F) -> Result<&Query<T>> {
        let mut result = self.result.borrow_mut();
        if result.is_none() {
            *result = Some(f());
        }
        result.as_ref().unwrap().as_ref().map(|_| self).map_err(|err| *err)
    }

    /// Takes ownership of the query result. Further attempts to take or peek the query
    /// result will panic unless it is returned by calling the `give` method.
    pub fn take(&self) -> T {
        self.result
            .borrow_mut()
            .take()
            .expect("missing query result")
            .unwrap()
    }

    /// Returns a stolen query result. Panics if there's already a result.
    pub fn give(&self, value: T) {
        let mut result = self.result.borrow_mut();
        assert!(result.is_none(), "a result already exists");
        *result = Some(Ok(value));
    }

    /// Borrows the query result using the RefCell. Panics if the result is stolen.
    pub fn peek(&self) -> Ref<'_, T> {
        Ref::map(self.result.borrow(), |r| {
            r.as_ref().unwrap().as_ref().expect("missing query result")
        })
    }

    /// Mutably borrows the query result using the RefCell. Panics if the result is stolen.
    pub fn peek_mut(&self) -> RefMut<'_, T> {
        RefMut::map(self.result.borrow_mut(), |r| {
            r.as_mut().unwrap().as_mut().expect("missing query result")
        })
    }
}

impl<T> Default for Query<T> {
    fn default() -> Self {
        Query {
            result: RefCell::new(None),
        }
    }
}

#[derive(Default)]
pub(crate) struct Queries {
    dep_graph_future: Query<Option<DepGraphFuture>>,
    dep_graph: Query<DepGraph>,
    codegen_channel: Query<(Steal<mpsc::Sender<Box<dyn Any + Send>>>,
                            Steal<mpsc::Receiver<Box<dyn Any + Send>>>)>,
    global_ctxt: Query<BoxedGlobalCtxt>,
    ongoing_codegen: Query<(Box<dyn Any>, Arc<OutputFilenames>)>,
    link: Query<()>,
}

impl Compiler {
    pub fn dep_graph_future(&self) -> Result<&Query<Option<DepGraphFuture>>> {
        self.queries.dep_graph_future.compute(|| {
            Ok(if self.session().opts.build_dep_graph() {
                Some(rustc_incremental::load_dep_graph(self.session()))
            } else {
                None
            })
        })
    }

    pub fn dep_graph(&self) -> Result<&Query<DepGraph>> {
        self.queries.dep_graph.compute(|| {
            Ok(match self.dep_graph_future()?.take() {
                None => DepGraph::new_disabled(),
                Some(future) => {
                    let (prev_graph, prev_work_products) =
                        time(self.session(), "blocked while dep-graph loading finishes", || {
                            future.open().unwrap_or_else(|e| rustc_incremental::LoadResult::Error {
                                message: format!("could not decode incremental cache: {:?}", e),
                            }).open(self.session())
                        });
                    DepGraph::new(prev_graph, prev_work_products)
                }
            })
        })
    }

    pub fn codegen_channel(&self) -> Result<&Query<(Steal<mpsc::Sender<Box<dyn Any + Send>>>,
                                                    Steal<mpsc::Receiver<Box<dyn Any + Send>>>)>> {
        self.queries.codegen_channel.compute(|| {
            let (tx, rx) = mpsc::channel();
            Ok((Steal::new(tx), Steal::new(rx)))
        })
    }

    pub fn global_ctxt(&self) -> Result<&Query<BoxedGlobalCtxt>> {
        self.queries.global_ctxt.compute(|| {
            let tx = self.codegen_channel()?.peek().0.steal();
            Ok(passes::create_global_ctxt(
                self,
                self.dep_graph()?.peek().clone(),
                self.io.clone(),
                tx
            ))
        })
    }

    pub fn ongoing_codegen(&self) -> Result<&Query<(Box<dyn Any>, Arc<OutputFilenames>)>> {
        self.queries.ongoing_codegen.compute(|| {
            let rx = self.codegen_channel()?.peek().1.steal();
            self.global_ctxt()?.peek_mut().enter(|tcx| {
                tcx.analysis(LOCAL_CRATE).ok();

                // Don't do code generation if there were any errors
                self.session().compile_status()?;

                let outputs = tcx.prepare_outputs(LocalCrate)?;

                Ok((passes::start_codegen(
                    &***self.codegen_backend(),
                    tcx,
                    rx,
                    &outputs,
                ), outputs))
            })
        })
    }

    pub fn link(&self) -> Result<&Query<()>> {
        self.queries.link.compute(|| {
            let sess = self.session();

            let (ongoing_codegen, outputs) = self.ongoing_codegen()?.take();

            self.codegen_backend().join_codegen_and_link(
                ongoing_codegen,
                sess,
                &*self.dep_graph()?.peek(),
                &outputs,
            ).map_err(|_| ErrorReported)?;

            Ok(())
        })
    }

    pub fn compile(&self) -> Result<()> {
        self.global_ctxt()?.peek_mut().enter(|tcx| {
            tcx.prepare_outputs(LocalCrate)?;
            Ok(())
        })?;

        if self.session().opts.output_types.contains_key(&OutputType::DepInfo)
            && self.session().opts.output_types.len() == 1
        {
            return Ok(())
        }

        // Drop AST after creating GlobalCtxt to free memory
        self.global_ctxt()?.peek_mut().enter(|tcx| {
            tcx.lower_ast_to_hir(LocalCrate)?;
            // Drop AST after lowering HIR to free memory
            mem::drop(tcx.expand_macros(LocalCrate).unwrap().ast_crate.steal());
            Ok(())
        })?;

        self.ongoing_codegen()?;

        // Drop GlobalCtxt after starting codegen to free memory
        mem::drop(self.global_ctxt()?.take());

        self.link().map(|_| ())
    }
}
