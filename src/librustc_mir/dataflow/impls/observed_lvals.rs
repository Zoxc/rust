// Copyright 2017 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

pub use super::*;

use rustc::mir::*;
use rustc::mir::visit::Visitor;
use dataflow::BitDenotation;

struct ObservedLvalueVisitor<'a, 'tcx: 'a, 'b, 'c: 'b> {
    data: &'a MaybeObservedLvals<'a, 'tcx>,
    sets: &'b mut BlockSets<'c, MovePathIndex>,
}

impl<'a, 'tcx, 'b, 'c> Visitor<'tcx> for ObservedLvalueVisitor<'a, 'tcx, 'b, 'c> {
    fn visit_rvalue(&mut self,
                    rvalue: &Rvalue<'tcx>,
                    location: Location) {
        if let Rvalue::Ref(_, _, ref lvalue) = *rvalue {
            // Mark the lvalue as observed
            on_lookup_result_bits(self.data.tcx, self.data.mir, &self.data.mdpe.move_data,
                                  self.data.mdpe.move_data.rev_lookup.find(lvalue),
                                  |moi| self.sets.gen(&moi));
        }

        self.super_rvalue(rvalue, location)
    }
}

#[derive(Copy, Clone)]
pub struct MaybeObservedLvals<'a, 'tcx: 'a> {
    tcx: TyCtxt<'a, 'tcx, 'tcx>,
    mir: &'a Mir<'tcx>,
    mdpe: &'a MoveDataParamEnv<'tcx>,
}

impl<'a, 'tcx: 'a> MaybeObservedLvals<'a, 'tcx> {
    pub fn new(tcx: TyCtxt<'a, 'tcx, 'tcx>,
               mir: &'a Mir<'tcx>,
               mdpe: &'a MoveDataParamEnv<'tcx>)
               -> Self {
        MaybeObservedLvals { tcx: tcx, mir: mir, mdpe: mdpe }
    }
}

impl<'a, 'tcx: 'a> HasMoveData<'tcx> for MaybeObservedLvals<'a, 'tcx> {
    fn move_data(&self) -> &MoveData<'tcx> { &self.mdpe.move_data }
}

impl<'a, 'tcx> BitDenotation for MaybeObservedLvals<'a, 'tcx> {
    type Idx = MovePathIndex;
    fn name() -> &'static str { "maybe_observed" }
    fn bits_per_block(&self) -> usize {
        self.move_data().move_paths.len()
    }

    fn start_block_effect(&self, _sets: &mut BlockSets<MovePathIndex>) {
        // Nothing is observed on function entry
    }

    fn statement_effect(&self,
                        sets: &mut BlockSets<MovePathIndex>,
                        loc: Location) {
        ObservedLvalueVisitor {
            data: self,
            sets,
        }.visit_statement(loc.block, &self.mir[loc.block].statements[loc.statement_index], loc);
    }

    fn terminator_effect(&self,
                         sets: &mut BlockSets<MovePathIndex>,
                         loc: Location) {
        ObservedLvalueVisitor {
            data: self,
            sets,
        }.visit_terminator(loc.block, self.mir[loc.block].terminator(), loc);
    }

    fn propagate_call_return(&self,
                             _in_out: &mut IdxSet<MovePathIndex>,
                             _call_bb: mir::BasicBlock,
                             _dest_bb: mir::BasicBlock,
                             _dest_lval: &mir::Lvalue) {
        // Nothing to do when a call returns successfully
    }
}

impl<'a, 'tcx> BitwiseOperator for MaybeObservedLvals<'a, 'tcx> {
    #[inline]
    fn join(&self, pred1: usize, pred2: usize) -> usize {
        pred1 | pred2 // "maybe" means we union effects of both preds
    }
}

impl<'a, 'tcx> DataflowOperator for MaybeObservedLvals<'a, 'tcx> {
    #[inline]
    fn bottom_value() -> bool {
        false // bottom = unobserved
    }
}

struct LvalObserversVisitor<'a, 'tcx: 'a, 'b, 'c: 'b> {
    data: &'a LvalObservers<'a, 'tcx>,
    sets: &'b mut BlockSets<'c, usize>,
}

impl<'a, 'tcx, 'b, 'c> Visitor<'tcx> for LvalObserversVisitor<'a, 'tcx, 'b, 'c> {
    fn visit_rvalue(&mut self,
                    rvalue: &Rvalue<'tcx>,
                    location: Location) {
        if let Rvalue::Ref(..) = *rvalue {
            if let Some(idx) = self.data.observers.iter().position(|e| e == &location) {
                self.sets.gen(&idx);
            }
        }

        self.super_rvalue(rvalue, location)
    }
}

#[derive(Clone)]
pub struct LvalObservers<'a, 'tcx: 'a> {
    tcx: TyCtxt<'a, 'tcx, 'tcx>,
    mir: &'a Mir<'tcx>,
    mdpe: &'a MoveDataParamEnv<'tcx>,
    filter: MovePathIndex,
    pub(crate) observers: Vec<Location>,
}

impl<'a, 'tcx> Visitor<'tcx> for LvalObservers<'a, 'tcx> {
    fn visit_rvalue(&mut self,
                    rvalue: &Rvalue<'tcx>,
                    location: Location) {
        if let Rvalue::Ref(_, _, ref lvalue) = *rvalue {
            on_lookup_result_bits(self.tcx, self.mir, &self.mdpe.move_data,
                                  self.mdpe.move_data.rev_lookup.find(lvalue),
                                  |moi| {
                if moi == self.filter {
                    self.observers.push(location);
                }
            });
        }

        self.super_rvalue(rvalue, location)
    }
}


impl<'a, 'tcx: 'a> LvalObservers<'a, 'tcx> {
    pub fn new(tcx: TyCtxt<'a, 'tcx, 'tcx>,
               mir: &'a Mir<'tcx>,
               mdpe: &'a MoveDataParamEnv<'tcx>,
               filter: MovePathIndex)
               -> Self {
        LvalObservers { tcx: tcx, mir: mir, mdpe: mdpe, filter, observers: Vec::new() }
    }
}

impl<'a, 'tcx: 'a> HasMoveData<'tcx> for LvalObservers<'a, 'tcx> {
    fn move_data(&self) -> &MoveData<'tcx> { &self.mdpe.move_data }
}

impl<'a, 'tcx> BitDenotation for LvalObservers<'a, 'tcx> {
    type Idx = usize;
    fn name() -> &'static str { "maybe_lval_observers" }
    fn bits_per_block(&self) -> usize {
        self.observers.len()
    }

    fn start_block_effect(&self, _sets: &mut BlockSets<usize>) {
        // Nothing is observed on function entry
    }

    fn statement_effect(&self,
                        sets: &mut BlockSets<usize>,
                        loc: Location) {
        LvalObserversVisitor {
            data: self,
            sets,
        }.visit_statement(loc.block, &self.mir[loc.block].statements[loc.statement_index], loc);
    }

    fn terminator_effect(&self,
                         sets: &mut BlockSets<usize>,
                         loc: Location) {
        LvalObserversVisitor {
            data: self,
            sets,
        }.visit_terminator(loc.block, self.mir[loc.block].terminator(), loc);
    }

    fn propagate_call_return(&self,
                             _in_out: &mut IdxSet<usize>,
                             _call_bb: mir::BasicBlock,
                             _dest_bb: mir::BasicBlock,
                             _dest_lval: &mir::Lvalue) {
        // Nothing to do when a call returns successfully
    }
}

impl<'a, 'tcx> BitwiseOperator for LvalObservers<'a, 'tcx> {
    #[inline]
    fn join(&self, pred1: usize, pred2: usize) -> usize {
        pred1 | pred2 // "maybe" means we union effects of both preds
    }
}

impl<'a, 'tcx> DataflowOperator for LvalObservers<'a, 'tcx> {
    #[inline]
    fn bottom_value() -> bool {
        false // bottom = unobserved
    }
}
