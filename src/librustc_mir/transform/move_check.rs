// Copyright 2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use dataflow::move_paths::{HasMoveData, MoveData};
use dataflow::{MaybeObservedLvals, LvalObservers};
use dataflow::{DataflowResults, BlockSets, BitDenotation};
use dataflow::{on_all_children_bits};
use dataflow::MoveDataParamEnv;
use dataflow;
use rustc::ty::TyCtxt;
use rustc::mir::visit::Visitor;
use rustc::mir::*;
use rustc::mir::transform::{MirPass, MirSource};
use rustc_data_structures::indexed_set::IdxSetBuf;
use rustc_data_structures::indexed_vec::Idx;

pub struct MoveCheck;

fn state_for_loc<T: BitDenotation>(loc: Location, analysis: &T, result: &DataflowResults<T>)
    -> IdxSetBuf<T::Idx> {
    let mut entry = result.sets().on_entry_set_for(loc.block.index()).to_owned();

    {
        let mut sets = BlockSets {
            on_entry: &mut entry.clone(),
            kill_set: &mut entry.clone(),
            gen_set: &mut entry,
        };

        for stmt in 0..loc.statement_index {
            let mut stmt_loc = loc;
            stmt_loc.statement_index = stmt;
            analysis.statement_effect(&mut sets, stmt_loc);
        }
    }

    entry
}

impl MirPass for MoveCheck {
    fn run_pass<'a, 'tcx>(&self,
                          tcx: TyCtxt<'a, 'tcx, 'tcx>,
                          src: MirSource,
                          mir: &mut Mir<'tcx>)
    {
        let id = src.item_id();
        let param_env = tcx.param_env(tcx.hir.local_def_id(id));
        let move_data = MoveData::gather_moves(mir, tcx, param_env, true);
        let mir = &*mir;
        let env = MoveDataParamEnv {
            move_data: move_data,
            param_env: param_env
        };
        let dead_unwinds = IdxSetBuf::new_empty(mir.basic_blocks().len());
        let analysis = MaybeObservedLvals::new(tcx, mir, &env);
        let observed =
            dataflow::do_dataflow(tcx, mir, id, &[], &dead_unwinds, analysis,
                                  |bd, p| &bd.move_data().move_paths[p]);

        for move_out in env.move_data.moves.iter() {
            let state = state_for_loc(move_out.source, &analysis, &observed);

            // FIXME: on_all_children_bits - lvalue_contents_drop_state_cannot_differ might
            // not be right here
            on_all_children_bits(tcx, mir, &env.move_data, move_out.path, |child| {
                let lvalue = &env.move_data.move_paths[child].lvalue;
                let lvalue_ty = lvalue.ty(mir, tcx).to_ty(tcx);
                let span = move_out.source.source_info(mir).span;
                if state.contains(&child) && !lvalue_ty.is_move(tcx.global_tcx(), param_env, span) {
                    let mut observers = LvalObservers::new(tcx, mir, &env, child);
                    observers.visit_mir(mir);

                    static STR: &'static &'static str = &"<>";

                    let observer_result =
                        dataflow::do_dataflow(tcx, mir, id, &[], &dead_unwinds, observers.clone(),
                                            |_, _| STR);

                    let state = state_for_loc(move_out.source, &observers, &observer_result);

                    let mut err = struct_span_err!(tcx.sess, span, E0901,
                        "cannot move value whose address is observed");

                    err.note(&format!("required because `{}` does not implement Move", lvalue_ty));

                    for (i, loc) in observers.observers.iter().enumerate() {
                        if state.contains(&i) {
                            span_note!(err,
                                       loc.source_info(mir).span,
                                       "the address was observed here");
                        }
                    }

                    err.emit();
                }
            });
        }
    }
}
