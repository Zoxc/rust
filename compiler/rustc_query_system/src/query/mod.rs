mod plumbing;
pub use self::plumbing::*;

mod job;
pub use self::job::{QueryInfo, QueryJob, QueryJobId};

mod caches;
pub use self::caches::{
    ArenaCacheSelector, CacheSelector, DefaultCacheSelector, QueryCache, QueryStorage,
};

mod config;
pub use self::config::{QueryAccessors, QueryConfig, QueryDescription};

use crate::dep_graph::{DepContext, DepGraph};

use rustc_data_structures::sync::Lock;
use rustc_data_structures::thin_vec::ThinVec;
use rustc_errors::Diagnostic;
use rustc_span::def_id::DefId;

pub trait QueryContext: DepContext {
    fn incremental_verify_ich(&self) -> bool;
    fn verbose(&self) -> bool;

    fn waiter_lock(&self) -> &Lock<()>;

    fn singlethreaded(&self) -> bool;

    /// Get string representation from DefPath.
    fn def_path_str(&self, def_id: DefId) -> String;

    /// Access the DepGraph.
    fn dep_graph(&self) -> &DepGraph<Self::DepKind>;

    /// Get the query information from the TLS context.
    fn current_query_job(&self) -> Option<QueryJobId<Self::DepKind>>;

    fn try_get_query_job(
        &self,
        id: QueryJobId<Self::DepKind>,
        block: bool,
    ) -> Option<QueryJob<Self::DepKind>>;
    fn try_get_query_info(&self, id: QueryJobId<Self::DepKind>, block: bool) -> Option<QueryInfo>;

    /// Executes a job by changing the `ImplicitCtxt` to point to the
    /// new query job while it executes. It returns the diagnostics
    /// captured during execution and the actual result.
    fn start_query<R>(
        &self,
        token: QueryJobId<Self::DepKind>,
        diagnostics: Option<&Lock<ThinVec<Diagnostic>>>,
        compute: impl FnOnce(Self) -> R,
    ) -> R;
}
