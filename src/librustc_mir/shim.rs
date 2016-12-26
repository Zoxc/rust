// Copyright 2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use rustc::hir;
use rustc::hir::def_id::DefId;
use rustc::infer;
use rustc::middle::const_val::ConstVal;
use rustc::mir::*;
use rustc::mir::transform::MirSource;
use rustc::traits::Reveal;
use rustc::ty::{self, Ty};
use rustc::ty::subst::{Kind, Subst, Substs};
use rustc::ty::maps::Providers;

use hair::cx::Cx;

use build;

use rustc_const_math::ConstInt;
use rustc_data_structures::indexed_vec::{IndexVec, Idx};

use syntax::abi::Abi;
use syntax::ast;
use syntax::symbol::keywords;
use syntax_pos::Span;

use std::fmt;
use std::iter;

use transform::{add_call_guards, no_landing_pads, simplify};
use util::elaborate_drops::{self, DropElaborator, DropStyle, DropFlagMode};
use util::patch::MirPatch;

pub fn provide(providers: &mut Providers) {
    providers.mir_shims = make_shim;
}

fn make_shim<'a, 'tcx>(tcx: ty::TyCtxt<'a, 'tcx, 'tcx>,
                       instance: ty::InstanceDef<'tcx>)
                       -> &'tcx Mir<'tcx>
{
    debug!("make_shim({:?})", instance);
    let did = instance.def_id();
    let param_env = tcx.parameter_environment(did);

    let mut result = match instance {
        ty::InstanceDef::Item(did) =>
            if tcx.is_generator(did) {
                build_generator_shim(tcx, did)
            } else {
                bug!("item {:?} passed to make_shim", instance)
            },
        ty::InstanceDef::Generator(..) =>
            bug!("generator {:?} passed to make_shim", instance),
        ty::InstanceDef::FnPtrShim(def_id, ty) => {
            let trait_ = tcx.trait_of_item(def_id).unwrap();
            let adjustment = match tcx.lang_items.fn_trait_kind(trait_) {
                Some(ty::ClosureKind::FnOnce) => Adjustment::Identity,
                Some(ty::ClosureKind::FnMut) |
                Some(ty::ClosureKind::Fn) => Adjustment::Deref,
                None => bug!("fn pointer {:?} is not an fn", ty)
            };
            // HACK: we need the "real" argument types for the MIR,
            // but because our substs are (Self, Args), where Args
            // is a tuple, we must include the *concrete* argument
            // types in the MIR. They will be substituted again with
            // the param-substs, but because they are concrete, this
            // will not do any harm.
            let sig = tcx.erase_late_bound_regions(&ty.fn_sig());
            let arg_tys = sig.inputs();

            build_call_shim(
                tcx,
                def_id,
                adjustment,
                CallKind::Indirect,
                Some(arg_tys)
            )
        }
        ty::InstanceDef::Virtual(def_id, _) => {
            // We are translating a call back to our def-id, which
            // trans::mir knows to turn to an actual virtual call.
            build_call_shim(
                tcx,
                def_id,
                Adjustment::Identity,
                CallKind::Direct(def_id),
                None
            )
        }
        ty::InstanceDef::ClosureOnceShim { call_once } => {
            let fn_mut = tcx.lang_items.fn_mut_trait().unwrap();
            let call_mut = tcx.global_tcx()
                .associated_items(fn_mut)
                .find(|it| it.kind == ty::AssociatedKind::Method)
                .unwrap().def_id;

            build_call_shim(
                tcx,
                call_once,
                Adjustment::RefMut,
                CallKind::Direct(call_mut),
                None
            )
        }
        ty::InstanceDef::DropGlue(def_id, ty) => {
            build_drop_shim(tcx, &param_env, def_id, ty)
        }
        ty::InstanceDef::Intrinsic(_) => {
            bug!("creating shims from intrinsics ({:?}) is unsupported", instance)
        }
    };
        debug!("make_shim({:?}) = untransformed {:?}", instance, result);
        no_landing_pads::no_landing_pads(tcx, &mut result);
        simplify::simplify_cfg(&mut result);
        add_call_guards::add_call_guards(&mut result);
    debug!("make_shim({:?}) = {:?}", instance, result);

    tcx.alloc_mir(result)
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Adjustment {
    Identity,
    Deref,
    RefMut,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum CallKind {
    Indirect,
    Direct(DefId),
}

fn temp_decl(mutability: Mutability, ty: Ty, span: Span) -> LocalDecl {
    LocalDecl {
        mutability, ty, name: None,
        source_info: SourceInfo { scope: ARGUMENT_VISIBILITY_SCOPE, span },
        is_user_variable: false
    }
}

fn local_decls_for_sig<'tcx>(sig: &ty::FnSig<'tcx>, span: Span)
    -> IndexVec<Local, LocalDecl<'tcx>>
{
    iter::once(temp_decl(Mutability::Mut, sig.output(), span))
        .chain(sig.inputs().iter().map(
            |ity| temp_decl(Mutability::Not, ity, span)))
        .collect()
}

fn build_drop_shim<'a, 'tcx>(tcx: ty::TyCtxt<'a, 'tcx, 'tcx>,
                             param_env: &ty::ParameterEnvironment<'tcx>,
                             def_id: DefId,
                             ty: Option<Ty<'tcx>>)
                             -> Mir<'tcx>
{
    debug!("build_drop_shim(def_id={:?}, ty={:?})", def_id, ty);

    // Check if this is a generator, if so, return the drop glue for it
    if let Some(&ty::TyS { sty: ty::TyGenerator(gen_def_id, substs), .. }) = ty {
        let mir = &**tcx.optimized_mir(gen_def_id).generator_drop.as_ref().unwrap();
        return mir.subst(tcx, substs.substs);
    }

    let substs = if let Some(ty) = ty {
        tcx.mk_substs(iter::once(Kind::from(ty)))
    } else {
        Substs::identity_for_item(tcx, def_id)
    };
    let fn_ty = tcx.type_of(def_id).subst(tcx, substs);
    let sig = tcx.erase_late_bound_regions(&fn_ty.fn_sig());
    let span = tcx.def_span(def_id);

    let source_info = SourceInfo { span, scope: ARGUMENT_VISIBILITY_SCOPE };

    let return_block = BasicBlock::new(1);
    let mut blocks = IndexVec::new();
    let block = |blocks: &mut IndexVec<_, _>, kind| {
        blocks.push(BasicBlockData {
            statements: vec![],
            terminator: Some(Terminator { source_info, kind }),
            is_cleanup: false
        })
    };
    block(&mut blocks, TerminatorKind::Goto { target: return_block });
    block(&mut blocks, TerminatorKind::Return);

    let mut mir = Mir::new(
        blocks,
        IndexVec::from_elem_n(
            VisibilityScopeData { span: span, parent_scope: None }, 1
        ),
        IndexVec::new(),
        sig.output(),
        None,
        local_decls_for_sig(&sig, span),
        sig.inputs().len(),
        vec![],
        span
    );

    if let Some(..) = ty {
        let patch = {
            let mut elaborator = DropShimElaborator {
                mir: &mir,
                patch: MirPatch::new(&mir),
                tcx, param_env
            };
            let dropee = Lvalue::Local(Local::new(1+0)).deref();
            let resume_block = elaborator.patch.resume_block();
            elaborate_drops::elaborate_drop(
                &mut elaborator,
                source_info,
                false,
                &dropee,
                (),
                return_block,
                Some(resume_block),
                START_BLOCK
            );
            elaborator.patch
        };
        patch.apply(&mut mir);
    }

    mir
}

pub struct DropShimElaborator<'a, 'tcx: 'a> {
    mir: &'a Mir<'tcx>,
    patch: MirPatch<'tcx>,
    tcx: ty::TyCtxt<'a, 'tcx, 'tcx>,
    param_env: &'a ty::ParameterEnvironment<'tcx>,
}

impl<'a, 'tcx> fmt::Debug for DropShimElaborator<'a, 'tcx> {
    fn fmt(&self, _f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        Ok(())
    }
}

impl<'a, 'tcx> DropElaborator<'a, 'tcx> for DropShimElaborator<'a, 'tcx> {
    type Path = ();

    fn patch(&mut self) -> &mut MirPatch<'tcx> { &mut self.patch }
    fn mir(&self) -> &'a Mir<'tcx> { self.mir }
    fn tcx(&self) -> ty::TyCtxt<'a, 'tcx, 'tcx> { self.tcx }
    fn param_env(&self) -> &'a ty::ParameterEnvironment<'tcx> { self.param_env }

    fn drop_style(&self, _path: Self::Path, mode: DropFlagMode) -> DropStyle {
        if let DropFlagMode::Shallow = mode {
            DropStyle::Static
        } else {
            DropStyle::Open
        }
    }

    fn get_drop_flag(&mut self, _path: Self::Path) -> Option<Operand<'tcx>> {
        None
    }

    fn clear_drop_flag(&mut self, _location: Location, _path: Self::Path, _mode: DropFlagMode) {
    }

    fn field_subpath(&self, _path: Self::Path, _field: Field) -> Option<Self::Path> {
        None
    }
    fn deref_subpath(&self, _path: Self::Path) -> Option<Self::Path> {
        None
    }
    fn downcast_subpath(&self, _path: Self::Path, _variant: usize) -> Option<Self::Path> {
        Some(())
    }
}

/// Build a "call" shim for `def_id`. The shim calls the
/// function specified by `call_kind`, first adjusting its first
/// argument according to `rcvr_adjustment`.
///
/// If `untuple_args` is a vec of types, the second argument of the
/// function will be untupled as these types.
fn build_call_shim<'a, 'tcx>(tcx: ty::TyCtxt<'a, 'tcx, 'tcx>,
                             def_id: DefId,
                             rcvr_adjustment: Adjustment,
                             call_kind: CallKind,
                             untuple_args: Option<&[Ty<'tcx>]>)
                             -> Mir<'tcx>
{
    debug!("build_call_shim(def_id={:?}, rcvr_adjustment={:?}, \
            call_kind={:?}, untuple_args={:?})",
           def_id, rcvr_adjustment, call_kind, untuple_args);

    let fn_ty = tcx.type_of(def_id);
    let sig = tcx.erase_late_bound_regions(&fn_ty.fn_sig());
    let span = tcx.def_span(def_id);

    debug!("build_call_shim: sig={:?}", sig);

    let mut local_decls = local_decls_for_sig(&sig, span);
    let source_info = SourceInfo { span, scope: ARGUMENT_VISIBILITY_SCOPE };

    let rcvr_arg = Local::new(1+0);
    let rcvr_l = Lvalue::Local(rcvr_arg);
    let mut statements = vec![];

    let rcvr = match rcvr_adjustment {
        Adjustment::Identity => Operand::Consume(rcvr_l),
        Adjustment::Deref => Operand::Consume(rcvr_l.deref()),
        Adjustment::RefMut => {
            // let rcvr = &mut rcvr;
            let ref_rcvr = local_decls.push(temp_decl(
                Mutability::Not,
                tcx.mk_ref(tcx.types.re_erased, ty::TypeAndMut {
                    ty: sig.inputs()[0],
                    mutbl: hir::Mutability::MutMutable
                }),
                span
            ));
            statements.push(Statement {
                source_info: source_info,
                kind: StatementKind::Assign(
                    Lvalue::Local(ref_rcvr),
                    Rvalue::Ref(tcx.types.re_erased, BorrowKind::Mut, rcvr_l)
                )
            });
            Operand::Consume(Lvalue::Local(ref_rcvr))
        }
    };

    let (callee, mut args) = match call_kind {
        CallKind::Indirect => (rcvr, vec![]),
        CallKind::Direct(def_id) => (
            Operand::Constant(box Constant {
                span: span,
                ty: tcx.type_of(def_id),
                literal: Literal::Value {
                    value: ConstVal::Function(def_id,
                        Substs::identity_for_item(tcx, def_id)),
                },
            }),
            vec![rcvr]
        )
    };

    if let Some(untuple_args) = untuple_args {
        args.extend(untuple_args.iter().enumerate().map(|(i, ity)| {
            let arg_lv = Lvalue::Local(Local::new(1+1));
            Operand::Consume(arg_lv.field(Field::new(i), *ity))
        }));
    } else {
        args.extend((1..sig.inputs().len()).map(|i| {
            Operand::Consume(Lvalue::Local(Local::new(1+i)))
        }));
    }

    let mut blocks = IndexVec::new();
    let block = |blocks: &mut IndexVec<_, _>, statements, kind, is_cleanup| {
        blocks.push(BasicBlockData {
            statements,
            terminator: Some(Terminator { source_info, kind }),
            is_cleanup
        })
    };

    // BB #0
    block(&mut blocks, statements, TerminatorKind::Call {
        func: callee,
        args: args,
        destination: Some((Lvalue::Local(RETURN_POINTER),
                           BasicBlock::new(1))),
        cleanup: if let Adjustment::RefMut = rcvr_adjustment {
            Some(BasicBlock::new(3))
        } else {
            None
        }
    }, false);

    if let Adjustment::RefMut = rcvr_adjustment {
        // BB #1 - drop for Self
        block(&mut blocks, vec![], TerminatorKind::Drop {
            location: Lvalue::Local(rcvr_arg),
            target: BasicBlock::new(2),
            unwind: None
        }, false);
    }
    // BB #1/#2 - return
    block(&mut blocks, vec![], TerminatorKind::Return, false);
    if let Adjustment::RefMut = rcvr_adjustment {
        // BB #3 - drop if closure panics
        block(&mut blocks, vec![], TerminatorKind::Drop {
            location: Lvalue::Local(rcvr_arg),
            target: BasicBlock::new(4),
            unwind: None
        }, true);

        // BB #4 - resume
        block(&mut blocks, vec![], TerminatorKind::Resume, true);
    }

    let mut mir = Mir::new(
        blocks,
        IndexVec::from_elem_n(
            VisibilityScopeData { span: span, parent_scope: None }, 1
        ),
        IndexVec::new(),
        sig.output(),
        None,
        local_decls,
        sig.inputs().len(),
        vec![],
        span
    );
    if let Abi::RustCall = sig.abi {
        mir.spread_arg = Some(Local::new(sig.inputs().len()));
    }
    mir
}

/// Build a "generator" shim for `def_id`. The shim constructs the generator
fn build_generator_shim<'a, 'tcx>(tcx: ty::TyCtxt<'a, 'tcx, 'tcx>,
                             def_id: DefId)
                             -> Mir<'tcx>
{
    debug!("build_generator_shim(def_id={:?})",
           def_id);

    let node_id = tcx.hir.as_local_node_id(def_id).unwrap();

    let sig = tcx.typeck_tables_of(def_id).liberated_fn_sigs[&node_id].clone();
    let span = tcx.def_span(def_id);

    debug!("build_generator_shim: sig={:?}", sig);

    let ty = tcx.type_of(def_id);
    let mut abi = sig.abi;
    let is_closure = if let ty::TyClosure(..) = ty.sty {
        // HACK(eddyb) Avoid having RustCall on closures,
        // as it adds unnecessary (and wrong) auto-tupling.
        abi = Abi::Rust;
        true
    } else {
        false
    };
    
    let mut local_decls = local_decls_for_sig(&sig, span);
    let source_info = SourceInfo { span, scope: ARGUMENT_VISIBILITY_SCOPE };

    let mut blocks = IndexVec::new();

    let mut statements = vec![];

    let mut args = vec![];

    let (substs, upvars) = if is_closure {
        let body_id = match tcx.hir.get(node_id) {
            hir::map::NodeExpr(expr) => {
                // FIXME(eddyb) Closures should have separate
                // function definition IDs and expression IDs.
                // Type-checking should not let closures get
                // this far in a constant position.
                // Assume that everything other than closures
                // is a constant "initializer" expression.
                match expr.node {
                    hir::ExprClosure(_, _, body, _) => body,
                    _ => bug!()
                }
            }
            _ => bug!()
        };

        let by_val = tcx.closure_kind(def_id) == ty::ClosureKind::FnOnce;

        let closure_by_val_ty = tcx.body_tables(body_id).node_id_to_type(node_id);

        let closure_ty = build::closure_self_ty(tcx, node_id, body_id);

        local_decls.raw.insert(1, LocalDecl {
            mutability: Mutability::Not,
            ty: closure_ty,
            name: None,
            source_info,
            is_user_variable: false,
        });

        let src = MirSource::from_node(tcx, node_id);
        let upvar_decls = tcx.infer_ctxt(body_id, Reveal::UserFacing).enter(|infcx| {
            let cx = Cx::new(&infcx, src);
            
            // Gather the upvars of a closure, if any.
            let upvar_decls: Vec<_> = tcx.with_freevars(node_id, |freevars| {
                freevars.iter().map(|fv| {
                    let var_id = tcx.hir.as_local_node_id(fv.def.def_id()).unwrap();
                    let by_ref = cx.tables().upvar_capture(ty::UpvarId {
                        var_id: var_id,
                        closure_expr_id: node_id
                    }).map_or(false, |capture| match capture {
                        ty::UpvarCapture::ByValue => false,
                        ty::UpvarCapture::ByRef(..) => true
                    });
                    let mut decl = UpvarDecl {
                        debug_name: keywords::Invalid.name(),
                        by_ref: by_ref
                    };
                    if let Some(hir::map::NodeLocal(pat)) = tcx.hir.find(var_id) {
                        if let hir::PatKind::Binding(_, _, ref ident, _) = pat.node {
                            decl.debug_name = ident.node;
                        }
                    }
                    decl
                }).collect()
            });

            upvar_decls
        });

        let (substs, upvar_tys) = match closure_by_val_ty.sty {
                ty::TyClosure(def_id, substs) => (substs, substs.upvar_tys(def_id, tcx)),
                 _ => bug!(),
        };

        for (i, ty) in upvar_tys.enumerate() {
            let mut base = Lvalue::Local(Local::new(1));
            if !by_val {
                base = Lvalue::Projection(Box::new(Projection {
                    base: base,
                    elem: ProjectionElem::Deref,
                }));
            }
            let field = Projection {
                base: base,
                elem: ProjectionElem::Field(Field::new(i), ty),
            };
            let arg = Lvalue::Projection(Box::new(field));
            args.push(Operand::Consume(arg));
        }

        (substs, upvar_decls)
    } else {
        let substs = ty::ClosureSubsts {
            substs: Substs::identity_for_item(tcx, def_id)
        };
        (substs, vec![])
    };

    args.push(Operand::Constant(box Constant {
        span: span,
        ty: tcx.types.u32,
        literal: Literal::Value {
            value: ConstVal::Integral(ConstInt::U32(0)),
        },
    }));

    let offset = if is_closure { 1 } else { 0 };
    for i in 0..sig.inputs().len() {
        let arg = Local::new(offset + i + 1);
        args.push(Operand::Consume(Lvalue::Local(arg)));
    }

    statements.push(Statement {
        source_info: source_info,
        kind: StatementKind::Assign(
            Lvalue::Local(RETURN_POINTER),
            Rvalue::Aggregate(box AggregateKind::Generator(def_id, substs), args),
        )
    });

    blocks.push(BasicBlockData {
            statements,
            terminator: Some(Terminator { source_info, kind: TerminatorKind::Return }),
            is_cleanup:false
    });

    let mut mir = Mir::new(
        blocks,
        IndexVec::from_elem_n(
            VisibilityScopeData { span: span, parent_scope: None }, 1
        ),
        IndexVec::new(),
        sig.output(),
        None,
        local_decls,
        sig.inputs().len() + offset,
        upvars,
        span
    );
    if let Abi::RustCall = abi {
        mir.spread_arg = Some(Local::new(sig.inputs().len() + offset));
    }
    use util;
    util::dump_mir(
        tcx,
        None,
        "gen_shim",
        &true,
        MirSource::Fn(node_id),
        &mir
    );
    mir
}

pub fn build_adt_ctor<'a, 'gcx, 'tcx>(infcx: &infer::InferCtxt<'a, 'gcx, 'tcx>,
                                      ctor_id: ast::NodeId,
                                      fields: &[hir::StructField],
                                      span: Span)
                                      -> (Mir<'tcx>, MirSource)
{
    let tcx = infcx.tcx;
    let def_id = tcx.hir.local_def_id(ctor_id);
    let sig = match tcx.type_of(def_id).sty {
        ty::TyFnDef(_, _, fty) => tcx.no_late_bound_regions(&fty)
            .expect("LBR in ADT constructor signature"),
        _ => bug!("unexpected type for ctor {:?}", def_id)
    };
    let sig = tcx.erase_regions(&sig);

    let (adt_def, substs) = match sig.output().sty {
        ty::TyAdt(adt_def, substs) => (adt_def, substs),
        _ => bug!("unexpected type for ADT ctor {:?}", sig.output())
    };

    debug!("build_ctor: def_id={:?} sig={:?} fields={:?}", def_id, sig, fields);

    let local_decls = local_decls_for_sig(&sig, span);

    let source_info = SourceInfo {
        span: span,
        scope: ARGUMENT_VISIBILITY_SCOPE
    };

    let variant_no = if adt_def.is_enum() {
        adt_def.variant_index_with_id(def_id)
    } else {
        0
    };

    // return = ADT(arg0, arg1, ...); return
    let start_block = BasicBlockData {
        statements: vec![Statement {
            source_info: source_info,
            kind: StatementKind::Assign(
                Lvalue::Local(RETURN_POINTER),
                Rvalue::Aggregate(
                    box AggregateKind::Adt(adt_def, variant_no, substs, None),
                    (1..sig.inputs().len()+1).map(|i| {
                        Operand::Consume(Lvalue::Local(Local::new(i)))
                    }).collect()
                )
            )
        }],
        terminator: Some(Terminator {
            source_info: source_info,
            kind: TerminatorKind::Return,
        }),
        is_cleanup: false
    };

    let mir = Mir::new(
        IndexVec::from_elem_n(start_block, 1),
        IndexVec::from_elem_n(
            VisibilityScopeData { span: span, parent_scope: None }, 1
        ),
        IndexVec::new(),
        sig.output(),
        None,
        local_decls,
        sig.inputs().len(),
        vec![],
        span
    );
    (mir, MirSource::Fn(ctor_id))
}
