// Copyright 2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Transforming generator bodies into a state machines

#![allow(warnings)]

use rustc::hir;
use rustc::hir::def_id::DefId;
use rustc::middle::const_val::ConstVal;
use rustc::middle::region::ROOT_CODE_EXTENT;
use rustc::mir::*;
use rustc::mir::transform::{MirPass, MirSource, Pass};
use rustc::mir::visit::{LvalueContext, MutVisitor};
use rustc::ty::{self, TyCtxt, AdtDef, Ty};
use rustc::ty::subst::{Kind, Substs};
use util::dump_mir;
use rustc_const_math::ConstInt;
use rustc_data_structures::bitvec::BitVector;
use rustc_data_structures::indexed_vec::Idx;
use std::collections::HashMap;
use std::borrow::Cow;
use std::iter::once;
use syntax::ast::NodeId;
use transform::simplify;

pub struct StateTransform;

impl Pass for StateTransform {}

struct RenameLocalVisitor {
    from: Local,
    to: Local,
}

impl<'tcx> MutVisitor<'tcx> for RenameLocalVisitor {
    fn visit_local(&mut self,
                        local: &mut Local) {
        if *local == self.from {
            *local = self.to;
        }
    }
}

struct SwapLocalVisitor {
    a: Local,
    b: Local,
}

impl<'tcx> MutVisitor<'tcx> for SwapLocalVisitor {
    fn visit_local(&mut self,
                        local: &mut Local) {
        if *local == self.a {
            *local = self.b;
        } else if *local == self.b {
            *local = self.a;
        }
    }
}

struct InsertLocalVisitor {
    local: Local,
}

impl<'tcx> MutVisitor<'tcx> for InsertLocalVisitor {
    fn visit_local(&mut self,
                        local: &mut Local) {
        if local.index() >= self.local.index() {
            *local = Local::new(local.index() + 1);
        }
    }
}

struct DerefArgVisitor;

impl<'tcx> MutVisitor<'tcx> for DerefArgVisitor {
    fn visit_lvalue(&mut self,
                    lvalue: &mut Lvalue<'tcx>,
                    context: LvalueContext<'tcx>,
                    location: Location) {
        if *lvalue == Lvalue::Local(Local::new(1)) {
            *lvalue = Lvalue::Projection(Box::new(Projection {
                base: lvalue.clone(),
                elem: ProjectionElem::Deref,
            }));
        }
    }
}

struct TransformVisitor<'a, 'tcx: 'a> {
    tcx: TyCtxt<'a, 'tcx, 'tcx>,
    state_adt_ref: &'tcx AdtDef,
    state_substs: &'tcx Substs<'tcx>,
    remap: HashMap<Local, (Ty<'tcx>, usize)>,
    bb_target_count: u32,
    bb_targets: HashMap<(BasicBlock, Option<BasicBlock>), u32>,
    new_ret_local: Local,
    return_block: BasicBlock,
    state_field: usize,
}

impl<'a, 'tcx> TransformVisitor<'a, 'tcx> {
    fn make_state(&self, idx: usize, val: Operand<'tcx>) -> Rvalue<'tcx> {
        let adt = AggregateKind::Adt(self.state_adt_ref, idx, self.state_substs, None);
        Rvalue::Aggregate(adt, vec![val])
    }

    fn make_field(&self, idx: usize, ty: Ty<'tcx>) -> Lvalue<'tcx> {
        let base = Lvalue::Local(Local::new(1));
        let base = Lvalue::Projection(Box::new(Projection {
            base: base,
            elem: ProjectionElem::Deref,
        }));
        let field = Projection {
            base: base,
            elem: ProjectionElem::Field(Field::new(idx), ty),
        };
        Lvalue::Projection(Box::new(field))
    }
/*
    fn make_unsafe(data: &mut Vec<BasicBlockData>, block: BasicBlock) {
        let find_split = ..;
        if some split {
            make_unsafe(data, split)
        }
    }*/
}

impl<'a, 'tcx> MutVisitor<'tcx> for TransformVisitor<'a, 'tcx> {
    fn visit_lvalue(&mut self,
                    lvalue: &mut Lvalue<'tcx>,
                    context: LvalueContext<'tcx>,
                    location: Location) {
        if let Lvalue::Local(l) = *lvalue {
            if let Some(&(ty, idx)) = self.remap.get(&l) {
                *lvalue = self.make_field(idx, ty);
            }
        } else {
            self.super_lvalue(lvalue, context, location);
        }
    }

    fn visit_basic_block_data(&mut self,
                              block: BasicBlock,
                              data: &mut BasicBlockData<'tcx>) {
        let ret_val = match data.terminator.as_ref().unwrap().kind {
            TerminatorKind::Return => Some((1,
                self.return_block,
                Operand::Consume(Lvalue::Local(self.new_ret_local)),
                None)),
            TerminatorKind::Suspend { ref value, resume, drop } => Some((0,
                resume,
                value.clone(),
                drop)),
            _ => None
        };

        data.retain_statements(|s| {
            match s.kind {
                StatementKind::StorageLive(ref l) | StatementKind::StorageDead(ref l) => {
                    if let Lvalue::Local(l) = *l {
                        !self.remap.contains_key(&l)
                    } else {
                        true
                    }
                }
                _ => true
            }
        });

        ret_val.map(|(state_idx, resume, v, drop)| {
            let bb_idx = {
                let bb_targets = &mut self.bb_targets;
                let bb_target = &mut self.bb_target_count;
                *bb_targets.entry((resume, drop)).or_insert_with(|| {
                    let target = *bb_target;
                    *bb_target = target.checked_add(1).unwrap();
                    target
                })
            };
            let source_info = data.terminator.as_ref().unwrap().source_info;
            let state = self.make_field(self.state_field, self.tcx.types.u32);
            let val = Operand::Constant(Constant {
                span: source_info.span,
                ty: self.tcx.types.u32,
                literal: Literal::Value {
                    value: ConstVal::Integral(ConstInt::U32(bb_idx)),
                },
            });
            data.statements.push(Statement {
                source_info,
                kind: StatementKind::Assign(state, Rvalue::Use(val)),
            });
            data.statements.push(Statement {
                source_info,
                kind: StatementKind::Assign(Lvalue::Local(RETURN_POINTER),
                    self.make_state(state_idx, v)),
            });
            data.terminator.as_mut().unwrap().kind = TerminatorKind::Return;
        });

        self.super_basic_block_data(block, data);
    }
}

fn get_body_id<'a, 'tcx>(tcx: TyCtxt<'a, 'tcx, 'tcx>, node_id: NodeId) -> (bool, hir::BodyId) {
    // Figure out what primary body this item has.
    match tcx.hir.get(node_id) {
        hir::map::NodeItem(item) => {
            match item.node {
                hir::ItemConst(_, body) |
                hir::ItemStatic(_, _, body) |
                hir::ItemFn(.., body) => (false, body),
                _ => bug!(),
            }
        }
        hir::map::NodeTraitItem(item) => {
            match item.node {
                hir::TraitItemKind::Const(_, Some(body)) |
                hir::TraitItemKind::Method(_,
                    hir::TraitMethod::Provided(body)) => (false, body),
                _ => bug!(),
            }
        }
        hir::map::NodeImplItem(item) => {
            match item.node {
                hir::ImplItemKind::Const(_, body) |
                hir::ImplItemKind::Method(_, body) => (false, body),
                _ => bug!(),
            }
        }
        hir::map::NodeExpr(expr) => {
            // FIXME(eddyb) Closures should have separate
            // function definition IDs and expression IDs.
            // Type-checking should not let closures get
            // this far in a constant position.
            // Assume that everything other than closures
            // is a constant "initializer" expression.
            match expr.node {
                hir::ExprClosure(_, _, body, _) => (true, body),
                _ => (false, hir::BodyId { node_id: expr.id })
            }
        }
        _ => bug!(),
    }
}

fn ensure_generator_state_argument<'a, 'tcx>(
                tcx: TyCtxt<'a, 'tcx, 'tcx>,
                node_id: NodeId,
                def_id: DefId,
                mir: &mut Mir<'tcx>) -> Ty<'tcx> {
    // Figure out what primary body this item has.
    let (is_closure, body_id) = get_body_id(tcx, node_id);

    let substs = if is_closure {
        let closure_ty = tcx.body_tables(body_id).node_id_to_type(node_id);
        if let ty::TyClosure(id, substs) = closure_ty.sty {
            assert!(id == def_id);
            substs
        } else {
            bug!()
        }
    } else {
        ty::ClosureSubsts {
            substs: Substs::identity_for_item(tcx, def_id)
        }
    };

    let gen_ty = tcx.mk_generator_from_closure_substs(def_id, substs);
    
    let region = ty::Region::ReFree(ty::FreeRegion {
        scope: tcx.region_maps.item_extent(body_id.node_id),
        bound_region: ty::BoundRegion::BrEnv,
    });

    let region = tcx.mk_region(region);

    let ref_gen_ty = tcx.mk_ref(region, ty::TypeAndMut {
        ty: gen_ty,
        mutbl: hir::MutMutable
    });

    let source_info = SourceInfo {
        span: mir.span,
        scope: ARGUMENT_VISIBILITY_SCOPE,
    };

    let gen_arg = LocalDecl {
        mutability: Mutability::Mut,
        ty: ref_gen_ty,
        name: None,
        source_info,
        is_user_variable: false,
    };

    if is_closure {
        // Replace the closure argument with the generator argument
        mir.local_decls.raw[2] = gen_arg;

        if let ty::ClosureKind::FnOnce = tcx.closure_kind(def_id) {
            // Add a deref to accesses of the closure's fields
            DerefArgVisitor.visit_mir(mir);
        }
    } else {
        // We must insert the argument to the generator
        mir.local_decls.raw.insert(2, gen_arg);

        InsertLocalVisitor {
            local: Local::new(2),
        }.visit_mir(mir);
    }

    // Swap generator and implicit argument
    SwapLocalVisitor {
        a: Local::new(1),
        b: Local::new(2),
    }.visit_mir(mir);

    mir.local_decls.raw[..].swap(1, 2);

    gen_ty
}

fn replace_result_variable<'tcx>(ret_ty: Ty<'tcx>,
                            mir: &mut Mir<'tcx>) -> Local {
    let source_info = SourceInfo {
        span: mir.span,
        scope: ARGUMENT_VISIBILITY_SCOPE,
    };

    let new_ret = LocalDecl {
        mutability: Mutability::Mut,
        ty: ret_ty,
        name: None,
        source_info,
        is_user_variable: false,
    };
    let new_ret_local = Local::new(mir.local_decls.len());
    mir.local_decls.push(new_ret);
    mir.local_decls.swap(0, new_ret_local.index());

    RenameLocalVisitor {
        from: RETURN_POINTER,
        to: new_ret_local,
    }.visit_mir(mir);

    new_ret_local
}

fn compute_layout<'a, 'tcx>(tcx: TyCtxt<'a, 'tcx, 'tcx>,
                            def_id: DefId,
                            mir: &mut Mir<'tcx>) -> HashMap<Local, (Ty<'tcx>, usize)> {
    let source_info = SourceInfo {
        span: mir.span,
        scope: ARGUMENT_VISIBILITY_SCOPE,
    };

    // Drain all locals leaving only the generator struct argument and
    // the generator implicit argument
    let mut vars: Vec<_> = mir.local_decls.drain(3..).collect();
    
    // Insert the state of the generator into the layout
    vars.push(LocalDecl {
        mutability: Mutability::Mut,
        ty: tcx.types.u32,
        name: None,
        source_info,
        is_user_variable: false,
    });

    let mut remap = HashMap::new();

    for (i, v) in vars.iter().enumerate() {
        remap.insert(Local::new(3 + i), (v.ty, mir.upvar_decls.len() + 1 + i));
    }

    let layout = GeneratorLayout {
        fields: vars
    };
    
    tcx.generator_layout.borrow_mut().insert(def_id, layout);

    remap
}

fn insert_entry_point<'tcx>(mir: &mut Mir<'tcx>,
                            block: BasicBlockData<'tcx>) {
    mir.basic_blocks_mut().raw.insert(0, block);

    let blocks = mir.basic_blocks_mut().iter_mut();

    for target in blocks.flat_map(|b| b.terminator_mut().successors_mut()) {
        *target = BasicBlock::new(target.index() + 1);
    }
}

fn convert_cleaup_blocks<'tcx>(mir: &mut Mir<'tcx>) {
    fn reachable_no_unwind<'tcx>(visited: &mut BitVector, mir: &mut Mir<'tcx>, block: BasicBlock) {
        if !visited.insert(block.index()) {
            return;
        }

        let succ: Vec<_> = mir.basic_blocks_mut()[block].terminator_mut().kind.successors_no_unwind_mut().into_iter().map(|s| *s).collect();

        for b in succ {
            reachable_no_unwind(visited, mir, b);
        }
    }

    // Find the blocks reachable without unwind paths

    let mut visited = BitVector::new(mir.basic_blocks().len());
    reachable_no_unwind(&mut visited, mir, BasicBlock::new(0));

    // Are any of the reachable blocks cleanup blocks?
    // If so, we want to duplicate them

    let to_dup: Vec<_> = visited.iter().map(|i| BasicBlock::new(i)).filter(|i| mir.basic_blocks()[*i].is_cleanup).collect();

    let mut map = HashMap::new();

    for b in to_dup {
        let di = BasicBlock::new(mir.basic_blocks().len());
        let mut db = mir.basic_blocks()[b].clone();
        db.is_cleanup = false;
        if let TerminatorKind::Resume = db.terminator().kind {
            db.terminator_mut().kind = TerminatorKind::Return; 
        }
        mir.basic_blocks_mut().push(db);
        map.insert(b, di);
    }

    // Remap references from non-unwind paths to the cloned blocks

    for i in visited.iter() {
        let b = BasicBlock::new(i);

        let succ: Vec<_> = mir.basic_blocks_mut()[b].terminator_mut().kind.successors_no_unwind_mut();

        for b in succ {
            if let Some(r) = map.get(b) {
                *b = *r;
            }
        }
    }
}

fn generate_drop<'a, 'tcx>(
                tcx: TyCtxt<'a, 'tcx, 'tcx>,
                transform: &TransformVisitor<'a, 'tcx>,
                node_id: NodeId,
                def_id: DefId,
                source: MirSource,
                gen_ty: Ty<'tcx>,
                mir: &mut Mir<'tcx>) {
    let source_info = SourceInfo {
        span: mir.span,
        scope: ARGUMENT_VISIBILITY_SCOPE,
    };

    mir.basic_blocks_mut().push(BasicBlockData {
        statements: Vec::new(),
        terminator: Some(Terminator {
            source_info,
            kind: TerminatorKind::Return,
        }),
        is_cleanup: false,
    });

    let cases: Vec<_> = transform.bb_targets.iter().filter_map(|(&(r, u), &s)| {
        u.map(|d| (s, d))
    }).collect();

    let switch = TerminatorKind::SwitchInt {
        discr: Operand::Consume(transform.make_field(transform.state_field, tcx.types.u32)),
        switch_ty: tcx.types.u32,
        values: Cow::from(cases.iter().map(|&(i, _)| {
                ConstInt::U32(i)
            }).collect::<Vec<_>>()),
        targets: cases.iter().map(|&(_, d)| d).chain(once(transform.return_block)).collect(),
    };

    insert_entry_point(mir, BasicBlockData {
        statements: Vec::new(),
        terminator: Some(Terminator {
            source_info,
            kind: switch,
        }),
        is_cleanup: false,
    });

    // Remove the implicit argument
    mir.arg_count = 1;
    mir.local_decls.raw.pop();
    
    // Replace the return variable
    let source_info = SourceInfo {
        span: mir.span,
        scope: ARGUMENT_VISIBILITY_SCOPE,
    };

    mir.return_ty = tcx.mk_nil();
    mir.local_decls[RETURN_POINTER] = LocalDecl {
        mutability: Mutability::Mut,
        ty: tcx.mk_nil(),
        name: None,
        source_info,
        is_user_variable: false,
    };

    // Change the generator argument from &mut to *mut
    mir.local_decls[Local::new(1)] = LocalDecl {
        mutability: Mutability::Mut,
        ty: tcx.mk_ptr(ty::TypeAndMut {
            ty: gen_ty,
            mutbl: hir::Mutability::MutMutable, 
        }),
        name: None,
        source_info,
        is_user_variable: false,
    };

    convert_cleaup_blocks(mir);
    simplify::remove_dead_blocks(mir);

    dump_mir(tcx, "generator_drop", &0, source, mir);
}

fn generate_resume<'a, 'tcx>(
                tcx: TyCtxt<'a, 'tcx, 'tcx>,
                transform: &TransformVisitor<'a, 'tcx>,
                node_id: NodeId,
                def_id: DefId,
                source: MirSource,
                mir: &mut Mir<'tcx>) {
    let param_env = tcx.construct_parameter_environment(mir.span, def_id, ROOT_CODE_EXTENT);
    let drop_arg = mir.local_decls.raw[2].ty.needs_drop(tcx, &param_env);

    let cleanup = if drop_arg {
        Some(BasicBlock::new(mir.basic_blocks().len() + 1))
    } else {
        None
    };

    let term = TerminatorKind::Assert {
        cond: Operand::Constant(Constant {
            span: mir.span,
            ty: tcx.types.bool,
            literal: Literal::Value {
                value: ConstVal::Bool(false),
            },
        }),
        expected: true,
        msg: AssertMessage::GeneratorResumedAfterReturn,
        target: transform.return_block,
        cleanup: cleanup,
    };

    let source_info = SourceInfo {
        span: mir.span,
        scope: ARGUMENT_VISIBILITY_SCOPE,
    };


    mir.basic_blocks_mut().push(BasicBlockData {
        statements: Vec::new(),
        terminator: Some(Terminator {
            source_info,
            kind: term,
        }),
        is_cleanup: false,
    });

    if drop_arg {
        let resume_block = BasicBlock::new(mir.basic_blocks().len() + 1);

        let term = TerminatorKind::Drop {
            location: Lvalue::Local(Local::new(2)),
            target: resume_block,
            unwind: None,
        };

        mir.basic_blocks_mut().push(BasicBlockData {
            statements: Vec::new(),
            terminator: Some(Terminator {
                source_info,
                kind: term,
            }),
            is_cleanup: true,
        });

        mir.basic_blocks_mut().push(BasicBlockData {
            statements: Vec::new(),
            terminator: Some(Terminator {
                source_info,
                kind: TerminatorKind::Resume,
            }),
            is_cleanup: true,
        });
    }

    let switch = TerminatorKind::SwitchInt {
        discr: Operand::Consume(transform.make_field(transform.state_field, tcx.types.u32)),
        switch_ty: tcx.types.u32,
        values: Cow::from(transform.bb_targets.values().map(|&i| {
                ConstInt::U32(i)
            }).collect::<Vec<_>>()),
        targets: transform.bb_targets.keys().map(|&(k, _)| k).chain(once(transform.return_block)).collect(),
    };

    insert_entry_point(mir, BasicBlockData {
        statements: Vec::new(),
        terminator: Some(Terminator {
            source_info,
            kind: switch,
        }),
        is_cleanup: false,
    });
    
    dump_mir(tcx, "generator_resume", &0, source, mir);
}

impl<'tcx> MirPass<'tcx> for StateTransform {
    fn run_pass<'a>(&mut self,
                    tcx: TyCtxt<'a, 'tcx, 'tcx>,
                    source: MirSource,
                    mir: &mut Mir<'tcx>) {
        let suspend_ty = if let Some(suspend_ty) = mir.suspend_ty {
            suspend_ty
        } else {
            // This only applies to generators
            return
        };

        assert!(mir.generator_drop.is_none());

        let node_id = source.item_id();
        let def_id = tcx.hir.local_def_id(source.item_id());

        let gen_ty = ensure_generator_state_argument(tcx, node_id, def_id, mir);

        let state_did = tcx.lang_items.gen_state().unwrap();
        let state_adt_ref = tcx.adt_def(state_did);
        let state_substs = tcx.mk_substs([Kind::from(suspend_ty),
            Kind::from(mir.return_ty)].iter());
        let ret_ty = tcx.mk_adt(state_adt_ref, state_substs);

        let new_ret_local = replace_result_variable(ret_ty, mir);

        let remap = compute_layout(tcx, def_id, mir);

        let return_block = BasicBlock::new(mir.basic_blocks().len());

        let state_field = mir.upvar_decls.len();

        let mut bb_targets = HashMap::new();
        bb_targets.insert((BasicBlock::new(0), None), 0);

        let mut transform = TransformVisitor {
            tcx,
            state_adt_ref,
            state_substs,
            remap,
            bb_target_count: 1,
            bb_targets,
            new_ret_local,
            return_block,
            state_field,
        };
        transform.visit_mir(mir);

        mir.return_ty = ret_ty;
        mir.suspend_ty = None;
        mir.arg_count = 2;
        mir.spread_arg = None;

        let mut drop_impl = mir.clone();

        generate_drop(tcx, &transform, node_id, def_id, source, gen_ty, &mut drop_impl);

        mir.generator_drop = Some(box drop_impl);

        generate_resume(tcx, &transform, node_id, def_id, source, mir);
    }
}
