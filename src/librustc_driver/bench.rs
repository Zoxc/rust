// Copyright 2018 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use rustc::ty::TyCtxt;
use rustc::util::common::time;
use rustc::ty::maps::{DummyId, DummyDefId, Providers};
use rustc::hir::def_id::{DefId, CrateNum, DefIndex};

pub fn bench<'tcx, F: FnMut(usize)>(tcx: TyCtxt<'_, 'tcx, 'tcx>, name: &str, mult: usize, mut f: F) {
    let warmup: usize = 10000;
    let timed: usize = 20000000 * mult;
    for i in 1..warmup {
        f(i);
    }
    time(tcx.sess, name, || {
        for i in warmup..(warmup + timed) {
            f(i);
        }
    });
}

fn def_id_from_usize(i: usize) -> DefId {
    DefId {
        krate: CrateNum::new(0),
        index: DefIndex::from_raw_u32(i as u32),
    }
}

pub fn run<'tcx>(tcx: TyCtxt<'_, 'tcx, 'tcx>) {
    if !tcx.sess.opts.debugging_opts.bench_queries {
        return;
    }
    bench(tcx, "hot query (usize)", 20, |_| {
        tcx.dummy_bench_query(DummyId(0));
    });
    bench(tcx, "cold query (usize)", 1, |i| {
        tcx.dummy_bench_query(DummyId(i));
    });
    bench(tcx, "hot query (dummy DefId)", 20, |_| {
        tcx.dummy_bench_query_dummy_def_id(DummyDefId(0, 0));
    });
    bench(tcx, "cold query (dummy DefId)", 1, |i| {
        tcx.dummy_bench_query_dummy_def_id(DummyDefId(0, i as u32));
    });
    bench(tcx, "hot query (DefId)", 20, |_| {
        tcx.dummy_bench_query_def_id(def_id_from_usize(0));
    });
    if !tcx.dep_graph.is_fully_enabled() {
        bench(tcx, "cold query (DefId)", 1, |i| {
            tcx.dummy_bench_query_def_id(def_id_from_usize(i));
        });
    }
}

#[cfg(not(any(target_arch = "asmjs", target_arch = "wasm32")))]
pub fn black_box<T>(dummy: T) -> T {
    // we need to "use" the argument in some way LLVM can't
    // introspect.
    unsafe { asm!("" : : "r"(&dummy)) }
    dummy
}
#[cfg(any(target_arch = "asmjs", target_arch = "wasm32"))]
#[inline(never)]
pub fn black_box<T>(dummy: T) -> T {
    dummy
}

pub fn provide(providers: &mut Providers) {
    providers.dummy_bench_query = |_, _| black_box(true);
    providers.dummy_bench_query_dummy_def_id = |_, _| black_box(true);
    providers.dummy_bench_query_def_id = |_, _| black_box(true);
}
