// Copyright 2017 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![allow(warnings)]

use std::mem;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use rustc_data_structures::sync::{Lock, LockGuard, Lrc, Weak};
use rayon_core::registry::{self, Registry, WorkerThread};
use rayon_core::fiber::{Fiber, Waitable, WaiterLatch};
use rayon_core::latch::{LatchProbe, Latch};
use syntax_pos::Span;
use ty::tls;
use ty::maps::Query;
use ty::maps::plumbing::CycleError;
use ty::context::TyCtxt;
use errors::Diagnostic;
use std::process;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct StackEntry<'tcx> {
    pub span: Span,
    pub query: Query<'tcx>,
    //pub id: usize,
}

#[derive(Hash, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ref<'tcx>(pub *const QueryJob<'tcx>);

unsafe impl<'tcx> Send for Ref<'tcx> {}
unsafe impl<'tcx> Sync for Ref<'tcx> {}

pub struct QueryJob<'tcx> {
    pub latch: Lock<(bool, Vec<(usize, Fiber, usize)>)>,
    pub entry: Option<StackEntry<'tcx>>,
    pub track_diagnostics: bool,
    pub diagnostics: Lock<Vec<Diagnostic>>,
    /// Used to prevent layout from recursing too deeply.
    pub layout_depth: usize,
    pub parent: Option<Ref<'tcx>>,
}

impl<'tcx> QueryJob<'tcx> {
    pub fn new(entry: Option<StackEntry<'tcx>>,
               track_diagnostics: bool,
               layout_depth: usize,
               parent: Option<Ref<'tcx>>) -> Self {
        QueryJob {
            latch: Lock::new((false, Vec::new())),
            track_diagnostics,
            diagnostics: Lock::new(Vec::new()),
            entry,
            layout_depth,
            parent,
        }
    }

    pub fn start(&self) {}

    pub fn await(&self) {
        #[cfg(parallel_queries)]
        registry::in_worker(|worker, _| {
            unsafe {
                worker.wait_enqueue(self);
            }
        });
    }

    pub fn signal_complete(&self) {
        #[cfg(parallel_queries)]
        {
            self.set(true);
        }
    }

    fn set(&self, signal: bool) {
        let mut data = self.latch.lock();
        // A query cycle may cause us to set this twice
        //debug_assert!(!data.0);
        data.0 = true;
        let waiters = !data.1.is_empty();
        for (worker_index, fiber, _tlv) in data.1.drain(..) {
            let registry = Registry::current();
            registry.resume_fiber(worker_index, fiber);
        }
        if waiters && signal {
            Registry::current().signal();
        }
    }
}

pub(super) enum QueryResult<'tcx, T> {
    Started(Lrc<QueryJob<'tcx>>),
    Complete(T),
    Cycle(CycleError<'tcx>),
    Poisoned,
}

struct OnPanic<F: Fn()>(F);

impl<F: Fn()> Drop for OnPanic<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

fn print_job<'a, 'tcx, 'lcx>(tcx: TyCtxt<'a, 'tcx, 'lcx>, job: &QueryJob<'tcx>) -> String {
    if let Some(entry) = job.entry.as_ref() {
        format!("[{}] {:x} {:?}",
            0/*entry.id*/, job as *const _ as usize, entry.query.describe(tcx))
    } else {
        format!("<top level>")
    }
}

fn proper_query<'tcx>(query: Ref<'tcx>) -> Option<Ref<'tcx>> {
    let mut query = unsafe { &*query.0 };
    loop {
        if query.entry.is_none() {
            if let Some(Ref(parent)) = query.parent {
                query = unsafe { &*parent };
            } else {
                return None;
            }
        } else {
            return Some(Ref(query))
        }
    }
}

fn visit_successors<'tcx, F, R>(query_ref: Ref<'tcx>, mut visit: F) -> Option<R>
where
    F: FnMut(Ref<'tcx>) -> Option<R>
{
    let query = unsafe { &*query_ref.0 };
    if let Some(parent) = query.parent.and_then(|q| proper_query(q)) {
        //eprintln!("visiting parent {:?} of query {:?}", parent, query_ref);
        if let Some(cycle) = visit(parent) {
            return Some(cycle);
        }
    }
    for &(_, _, tlv) in query.latch.lock().1.iter() {
        let waiter_tls_cx = unsafe { &*(tlv as *const tls::ImplicitCtxt) };
        let mut query = &*waiter_tls_cx.query;
        if let Some(waiter) = proper_query(Ref(query)) {
            //eprintln!("visiting waiter {:?} of query {:?}", waiter, query_ref);
            if let Some(cycle) = visit(waiter) {
                return Some(cycle);
            }
        }
    }
    None
}

fn cycle_check<'tcx>(query: Ref<'tcx>,
                     stack: &mut Vec<Ref<'tcx>>,
                     visited: &mut HashSet<Ref<'tcx>>) -> Option<()> {
    if visited.contains(&query) {
        //eprintln!("visited query {:?} already for cycle {:#?}", query, stack);

        return if let Some(_) = stack.iter().position(|&q| q == query) {
        //eprintln!("[found on stack] visited query {:?} already for cycle {:#?}", query, stack);
            Some(())
        } else {
        /*eprintln!("[not found on stack] visited query {:?} already for cycle {:#?}",
            query, stack);*/
            None
        }
    }

    //eprintln!("looking for cycle {:#?} in query {:?}", stack, query);

    visited.insert(query);
    stack.push(query);

    let r = visit_successors(query, |successor| {
        //eprintln!("found successor {:?} in query {:?}", successor, query);
        cycle_check(successor, stack, visited)
    });

    //eprintln!("result for query {:?} {:?}", query, r);

    if r.is_none() {
        stack.pop();
    }

    r
}

fn query_entry<'tcx>(r: Ref<'tcx>) -> StackEntry<'tcx> {
    unsafe { (*r.0).entry.as_ref().unwrap().clone() }
}

pub fn deadlock(gcx_ptr: usize) {
    let on_panic = OnPanic(|| {
        eprintln!("deadlock handler panicked, aborting process");
        process::abort();
    });

    //eprintln!("saw rayon deadlock");
    unsafe { tls::with_global_query(gcx_ptr, |tcx| {
        /*let jobs = tcx.maps.collect_active_jobs();

        for job in jobs {
            eprintln!("still active query: {}", print_job(tcx, &*job));
            if let Some(Ref(parent)) = job.parent {
                let parent = unsafe { &*parent };
                eprintln!("   - has parent: {}", print_job(tcx, parent));
            }
            for &(_, _, tlv) in job.latch.lock().1.iter() {
                let waiter_tls_cx = unsafe { &*(tlv as *const tls::ImplicitCtxt) };
                let mut query = &*waiter_tls_cx.query;
                let mut i = 0;
                loop {
                    eprintln!("   - has waiter d{}: {}", i, print_job(tcx, query));
                    i += 1;
                    if let Some(Ref(parent)) = query.parent {
                        query = unsafe { &*parent };
                    } else {
                        break;
                    }
                }
            }
        }
*/
        let mut jobs: Vec<_> = tcx.maps.collect_active_jobs().iter().map(|j| Ref(&**j)).collect();

        while jobs.len() > 0 {
            let mut visited = HashSet::new();
            let mut stack = Vec::new();
            if cycle_check(jobs.pop().unwrap(), &mut stack, &mut visited).is_some() {
                //eprintln!("found cycle {:#?}", stack);
                let error = CycleError {
                    span: query_entry(stack[0]).span,
                    cycle: stack.iter().map(|&r| query_entry(r)).collect(),
                };
                for r in &stack {
                    jobs.remove_item(r);
                    let query = unsafe { &*r.0 };
                    let mut stack = stack.clone();
                    while stack[0] != *r {
                        let last = stack.pop().unwrap();
                        stack.insert(0, last);
                    }
                    let error = CycleError {
                        span: query_entry(stack[0]).span,
                        cycle: stack.iter().map(|&r| query_entry(r)).collect(),
                    };
                    tcx.maps.set_cycle_error(query.entry.as_ref().unwrap().query.clone(),
                                             error);
                    query.set(false);
                }
                /*tcx.report_cycle(CycleError {
                    span: query_entry(stack[0]).span,
                    cycle: stack.iter().map(|&r| query_entry(r)).collect(),
                }).emit();*/
            }
        }

        //tcx.sess.parse_sess.span_diagnostic.flush_buffered_diagnostic();
    })};
    //eprintln!("aborting due to deadlock");
    //process::abort();
    mem::forget(on_panic);
    Registry::current().signal();
}

impl<'tcx> LatchProbe for QueryJob<'tcx> {
    #[inline]
    fn probe(&self) -> bool {
        self.latch.lock().0
    }
}

impl<'tcx> Latch for QueryJob<'tcx> {
    fn set(&self) {
        self.set(true);
    }
}

impl<'tcx> Waitable for QueryJob<'tcx> {
    fn complete(&self, _worker_thread: &WorkerThread) -> bool {
        self.probe()
    }

    fn await(&self, worker_thread: &WorkerThread, waiter: Fiber, tlv: usize) {
        let mut data = self.latch.lock();
        if data.0 {
            worker_thread.registry.resume_fiber(worker_thread.index(), waiter);
        } else {
            data.1.push((worker_thread.index(), waiter, tlv));
        }
    }
}
