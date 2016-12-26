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

use rustc::middle::const_val::ConstVal;
use rustc::mir::*;
use rustc::mir::transform::{MirPass, MirSource, Pass};
use rustc::mir::visit::{LvalueContext, MutVisitor};
use rustc::ty::{TyCtxt, AdtDef, Ty};
use rustc::ty::subst::{Kind, Substs};
use util::dump_mir;
use rustc_const_math::ConstInt;
use rustc_data_structures::indexed_vec::Idx;
use std::collections::HashMap;
use std::borrow::Cow;
use std::iter::once;

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

fn const_int(block: BasicBlock) -> ConstInt {
    // FIXME: to u32 conversion
    ConstInt::U32(block.index() as u32)
}

struct TransformVisitor<'a, 'tcx: 'a> {
    tcx: TyCtxt<'a, 'tcx, 'tcx>,
    state_adt_ref: &'tcx AdtDef,
    state_substs: &'tcx Substs<'tcx>,
    remap: HashMap<Local, (Ty<'tcx>, usize)>,
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
                Operand::Consume(Lvalue::Local(self.new_ret_local)))),
            TerminatorKind::Suspend { ref value, resume, ..} => Some((0,
                resume,
                value.clone())),
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

        ret_val.map(|(idx, resume, v)| {
            let source_info = data.terminator.as_ref().unwrap().source_info;
            let state = self.make_field(self.state_field, self.tcx.types.u32);
            let val = Operand::Constant(Constant {
                span: source_info.span,
                ty: self.tcx.types.u32,
                literal: Literal::Value {
                    value: ConstVal::Integral(const_int(resume)),
                },
            });
            data.statements.push(Statement {
                source_info,
                kind: StatementKind::Assign(state, Rvalue::Use(val)),
            });
            data.statements.push(Statement {
                source_info,
                kind: StatementKind::Assign(Lvalue::Local(RETURN_POINTER),
                    self.make_state(idx, v)),
            });
            data.terminator.as_mut().unwrap().kind = TerminatorKind::Return;
        });

        self.super_basic_block_data(block, data);
    }
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

        let state_did = tcx.lang_items.gen_state().unwrap();
        let state_adt_ref = tcx.lookup_adt_def(state_did);
        let state_substs = tcx.mk_substs([Kind::from(suspend_ty),
            Kind::from(mir.return_ty)].iter());
        let ret_ty = tcx.mk_adt(state_adt_ref, state_substs);

        let new_ret = LocalDecl {
            mutability: Mutability::Mut,
            ty: ret_ty,
            name: None,
            source_info: None,
        };
        let new_ret_local = Local::new(mir.local_decls.len());
        mir.local_decls.push(new_ret);
        mir.local_decls.swap(0, new_ret_local.index());

        RenameLocalVisitor {
            from: RETURN_POINTER,
            to: new_ret_local,
        }.visit_mir(mir);

        let mut remap = HashMap::new();
        let mut vars: Vec<_> = mir.local_decls.drain(2..).collect();
        
        vars.push(LocalDecl {
            mutability: Mutability::Mut,
            ty: tcx.types.u32,
            name: None,
            source_info: None,
        });

        for (i, v) in vars.iter().enumerate() {
            remap.insert(Local::new(2 + i), (v.ty, mir.upvar_decls.len() + 1 + i));
        }

        let layout = GeneratorLayout {
            fields: vars
        };
        
        tcx.generator_layout.borrow_mut().insert(tcx.hir.local_def_id(source.item_id()), layout);

        let return_block = BasicBlock::new(mir.basic_blocks().len());

        let state_field = mir.upvar_decls.len();

        let mut transform = TransformVisitor {
            tcx,
            state_adt_ref,
            state_substs,
            remap,
            new_ret_local,
            return_block,
            state_field,

        };
        transform.visit_mir(mir);

        mir.return_ty = ret_ty;
        mir.suspend_ty = None;
        mir.arg_count = 1;

        let source_info = SourceInfo {
            span: mir.span,
            scope: ARGUMENT_VISIBILITY_SCOPE,
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
            target: return_block,
            cleanup: None,
        };

        mir.basic_blocks_mut().push(BasicBlockData {
            statements: Vec::new(),
            terminator: Some(Terminator {
                source_info,
                kind: term,
            }),
            is_cleanup: false,
        });

        let switch = TerminatorKind::SwitchInt {
            discr: Operand::Consume(transform.make_field(state_field, tcx.types.u32)),
            switch_ty: tcx.types.u32,
            values: Cow::from(mir.basic_blocks().indices().map(|i| {
                    const_int(i)
                }).collect::<Vec<_>>()),
            targets: mir.basic_blocks().indices().chain(once(return_block)).collect(),
        };
        
        mir.basic_blocks_mut().raw.insert(0, BasicBlockData {
            statements: Vec::new(),
            terminator: Some(Terminator {
                source_info,
                kind: switch,
            }),
            is_cleanup: false,
        });

        {
            let blocks = mir.basic_blocks_mut().iter_mut();
            for target in blocks.flat_map(|b| b.terminator_mut().successors_mut()) {
                *target = BasicBlock::new(target.index() + 1);
            }
        }

        dump_mir(tcx, "generator_transform", &0, source, mir);
    }
}
