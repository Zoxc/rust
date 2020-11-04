use super::QueryContext;
use crate::query::plumbing::{CycleError, CycleErrorEntry};
use parking_lot::{Condvar, Mutex};
use rustc_data_structures::fx::FxHashSet;
use rustc_data_structures::jobserver;
use rustc_data_structures::sync::Lrc;
use rustc_span::Span;
use rustc_span::DUMMY_SP;
use std::borrow::Cow;
use std::convert::TryFrom;
use std::hash::Hash;
use std::iter::FromIterator;
use std::mem;
use std::num::NonZeroU32;
use std::time::Duration;

/// Information used to report a query cycle or print the query stack
#[derive(Clone, Debug)]
pub struct QueryInfo {
    pub name: &'static str,
    pub description: Cow<'static, str>,
    pub default_span: Span,
}

/// A value uniquely identifiying an active query job within a shard in the query cache.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct QueryShardJobId(pub NonZeroU32);

/// A value uniquely identifiying an active query job.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct QueryJobId<D> {
    /// Which job within a shard is this
    pub job: QueryShardJobId,

    /// In which shard is this job
    pub shard: u16,

    /// What kind of query this job is.
    pub kind: D,
}

impl<D> QueryJobId<D>
where
    D: Copy + Clone + Eq + Hash,
{
    pub fn new(job: QueryShardJobId, shard: usize, kind: D) -> Self {
        QueryJobId { job, shard: u16::try_from(shard).unwrap(), kind }
    }
}

/// Represents an active query job.
#[derive(Clone)]
pub struct QueryJob<D> {
    pub id: QueryShardJobId,

    /// The span corresponding to the reason for which this query was required.
    pub span: Span,

    /// The parent query job which created this job and is implicitly waiting on it.
    pub parent: Option<QueryJobId<D>>,

    /// The latch that is used to wait on this job.
    latch: Option<QueryLatch<D>>,
}

impl<D> QueryJob<D>
where
    D: Copy + Clone + Eq + Hash,
{
    /// Creates a new query job.
    pub fn new(id: QueryShardJobId, span: Span, parent: Option<QueryJobId<D>>) -> Self {
        QueryJob { id, span, parent, latch: None }
    }

    pub(super) fn latch(&mut self, id: QueryJobId<D>) -> QueryLatch<D> {
        if self.latch.is_none() {
            self.latch = Some(QueryLatch::new(id));
        }
        self.latch.as_ref().unwrap().clone()
    }

    /// Signals to waiters that the query is complete.
    ///
    /// This does nothing for single threaded rustc,
    /// as there are no concurrent jobs which could be waiting on us
    pub fn signal_complete(self) {
        #[cfg(parallel_compiler)]
        {
            if let Some(latch) = self.latch {
                latch.set();
            }
        }
    }
}

struct QueryWaiter<D> {
    query: Option<QueryJobId<D>>,
    condvar: Condvar,
    span: Span,
    can_timeout: bool,
}

impl<D> QueryWaiter<D> {
    fn notify(&self) {
        self.condvar.notify_one();
    }
}

struct QueryLatchInfo<D> {
    complete: bool,
    waiters: Vec<Lrc<QueryWaiter<D>>>,
}

#[derive(Clone)]
pub(super) struct QueryLatch<D> {
    pub(super) id: QueryJobId<D>,
    info: Lrc<Mutex<QueryLatchInfo<D>>>,
}

impl<D: Eq + Hash> QueryLatch<D> {
    fn new(id: QueryJobId<D>) -> Self {
        QueryLatch {
            id,
            info: Lrc::new(Mutex::new(QueryLatchInfo { complete: false, waiters: Vec::new() })),
        }
    }
}

impl<D> QueryLatch<D> {
    /// Awaits the caller on this latch by blocking the current thread.
    pub(super) fn wait_on(
        &self,
        query: Option<QueryJobId<D>>,
        span: Span,
        timeout: bool,
        before_wait: impl FnOnce(),
    ) -> bool {
        let waiter =
            Lrc::new(QueryWaiter { query, span, condvar: Condvar::new(), can_timeout: timeout });

        let mut info = self.info.lock();

        before_wait();

        // We push the waiter on to the `waiters` list. It can be accessed inside
        // the `wait` call below, by 1) the `set` method or 2) by deadlock detection.
        // Both of these will remove it from the `waiters` list before resuming
        // this thread.
        info.waiters.push(waiter.clone());

        if !info.complete {
            jobserver::release_thread();

            if timeout {
                waiter.condvar.wait_for(&mut info, Duration::from_millis(10));
            } else {
                waiter.condvar.wait(&mut info)
            }

            let complete = info.complete;

            // Release the lock before we potentially block in `acquire_thread`
            mem::drop(info);
            jobserver::acquire_thread();

            complete
        } else {
            true
        }
    }

    /// Sets the latch and resumes all waiters on it
    fn set(&self) {
        let mut info = self.info.lock();
        debug_assert!(!info.complete);
        info.complete = true;
        for waiter in info.waiters.drain(..) {
            waiter.notify();
        }
    }
}

/// Visits all the non-resumable and resumable waiters of a query.
/// Only waiters in a query are visited.
/// `visit` is called for every waiter and is passed a query waiting on `query_ref`
/// and a span indicating the reason the query waited on `query_ref`.
/// If `visit` returns `true`, this function returns `true`.
/// For visits of non-resumable waiters it returns the return value of `visit`.
/// For visits of resumable waiters it returns Some(Some(Waiter)) which has the
/// required information to resume the waiter.
/// If all `visit` calls returns None, this function also returns None.
fn visit_waiters<CTX: QueryContext>(
    tcx: CTX,
    query: QueryJobId<CTX::DepKind>,
    mut visit: impl FnMut(Span, QueryJobId<CTX::DepKind>) -> bool,
) -> bool {
    let job = if let Some(job) = tcx.try_get_query_job(query, true) { job } else { return false };

    // Visit the parent query which is a non-resumable waiter since it's on the same stack
    if let Some(parent) = job.parent {
        if visit(job.span, parent) {
            return true;
        }
    }

    // Visit the explicit waiters which use condvars and are resumable
    if let Some(latch) = job.latch {
        // Collect all the waiters so we don't call `visit` while locked.
        let waiters: Vec<_> = latch
            .info
            .lock()
            .waiters
            .iter()
            .filter_map(|waiter| {
                if waiter.can_timeout {
                    // Ignore waiters that can timeout since that could result in multiple
                    // queries in a cycle detecting a query cycle resulting in duplicate
                    // error messages.
                    None
                } else {
                    waiter.query.map(|query| (waiter.span, query))
                }
            })
            .collect();

        for (span, query) in waiters {
            if visit(span, query) {
                return true;
            }
        }
    }

    false
}

/// Look for query cycles by doing a depth first search starting at `query`.
/// `span` is the reason for the `query` to execute. This is initially DUMMY_SP.
/// If a cycle is detected, this initial value is replaced with the span causing
/// the cycle.
fn cycle_check<CTX: QueryContext>(
    tcx: CTX,
    query: QueryJobId<CTX::DepKind>,
    span: Span,
    stack: &mut Vec<(Span, QueryJobId<CTX::DepKind>)>,
    visited: &mut FxHashSet<QueryJobId<CTX::DepKind>>,
) -> bool {
    if !visited.insert(query) {
        return if let Some(p) = stack.iter().position(|q| q.1 == query) {
            // We detected a query cycle, fix up the initial span and return true

            // Remove previous stack entries
            stack.drain(0..p);
            // Replace the span for the first query with the cycle cause
            stack[0].0 = span;
            true
        } else {
            false
        };
    }

    // Query marked as visited is added to the stack
    stack.push((span, query));

    // Visit all the waiters
    let r = visit_waiters(tcx, query, |span, successor| {
        cycle_check(tcx, successor, span, stack, visited)
    });

    // Remove the entry in our stack if we didn't find a cycle
    if !r {
        stack.pop();
    }

    r
}

/// Finds out if there's a path to the compiler root (aka. code which isn't in a query)
/// from `query` without going through any of the queries in `visited`.
/// This is achieved with a depth first search.
fn connected_to_root<CTX: QueryContext>(
    tcx: CTX,
    query: QueryJobId<CTX::DepKind>,
    visited: &mut FxHashSet<QueryJobId<CTX::DepKind>>,
) -> bool {
    // We already visited this or we're deliberately ignoring it
    if !visited.insert(query) {
        return false;
    }

    let job = if let Some(job) = tcx.try_get_query_job(query, true) { job } else { return false };

    // This query is connected to the root (it has no query parent), return true
    if job.parent.is_none() {
        return true;
    }

    visit_waiters(tcx, query, |_, successor| connected_to_root(tcx, successor, visited))
}

pub fn check_for_cycle<CTX: QueryContext>(
    tcx: CTX,
    span: Span,
    current: Option<QueryJobId<CTX::DepKind>>,
    waiting_on: QueryJobId<CTX::DepKind>,
) -> Option<CycleError> {
    let current = if let Some(current) = current { current } else { return None };

    let mut visited = FxHashSet::default();
    let mut stack = Vec::new();

    // We're looking to see if `waiting_on` already depends on the current query.
    // Mark it as visited already
    visited.insert(waiting_on);
    stack.push((DUMMY_SP, waiting_on));

    // (span, current) is a waiter for `waiting_on`. Walk the graph starting from
    // that edge and see if we see `waiting_on` again.
    if cycle_check(tcx, current, span, &mut stack, &mut visited) {
        // The stack is a vector of pairs of spans and queries; reverse it so that
        // the earlier entries require later entries
        let (mut spans, queries): (Vec<_>, Vec<_>) = stack.into_iter().rev().unzip();

        // Shift the spans so that queries are matched with the span for their waitee
        spans.rotate_right(1);

        // Zip them back together
        let mut stack: Vec<_> = spans.into_iter().zip(queries).collect();

        // Find a query in the cycle which is connected to queries outside the cycle
        let (entry_point, usage) = stack
            .iter()
            .filter_map(|&(_, query)| {
                let query_job = tcx.try_get_query_job(query, true).unwrap();
                if query_job.parent.is_none() {
                    // This query is connected to the root (it has no query parent)
                    Some((query, None))
                } else {
                    let mut connected_waiter = None;

                    // Find a waiter which leads to the root
                    visit_waiters(tcx, query, |span, waiter| {
                        // Mark all the other queries in the cycle as already visited
                        let mut visited = FxHashSet::from_iter(stack.iter().map(|q| q.1));

                        if connected_to_root(tcx, waiter, &mut visited) {
                            connected_waiter = Some((span, waiter));
                        }

                        false
                    });

                    connected_waiter.map(|waiter| (query, Some(waiter)))
                }
            })
            .next()
            .unwrap();

        // Shift the stack so that our entry point is first
        let entry_point_pos = stack.iter().position(|(_, query)| query == &entry_point);
        if let Some(pos) = entry_point_pos {
            stack.rotate_left(pos);
        }

        let usage = usage.map(|(span, id)| CycleErrorEntry {
            span,
            query: tcx.try_get_query_info(id, true).unwrap(),
        });

        // Create the cycle error
        let error = CycleError {
            usage,
            cycle: stack
                .iter()
                .map(|&(span, q)| CycleErrorEntry {
                    span,
                    query: tcx.try_get_query_info(q, true).unwrap(),
                })
                .collect(),
        };

        Some(error)
    } else {
        None
    }
}

// FIXME: Find a new place for this
// Check that a cycle was found. It is possible for a deadlock to occur without
// a query cycle if a query which can be waited on uses Rayon to do multithreading
// internally. Such a query (X) may be executing on 2 threads (A and B) and A may
// wait using Rayon on B. Rayon may then switch to executing another query (Y)
// which in turn will wait on X causing a deadlock. We have a false dependency from
// X to Y due to Rayon waiting and a true dependency from Y to X. The algorithm here
// only considers the true dependency and won't detect a cycle.
