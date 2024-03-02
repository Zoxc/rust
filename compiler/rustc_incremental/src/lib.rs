//! Support for serializing the dep-graph and reloading it.

// tidy-alphabetical-start
#![allow(internal_features)]
#![cfg_attr(doc, recursion_limit = "256")] // FIXME(nnethercote): will be removed by #124141
#![deny(missing_docs)]
#![doc(html_root_url = "https://doc.rust-lang.org/nightly/nightly-rustc/")]
#![doc(rust_logo)]
#![feature(file_buffered)]
#![feature(rustdoc_internals)]
// tidy-alphabetical-end

mod assert_dep_graph;
mod errors;
mod persist;

pub use persist::{
    LoadResult, copy_cgu_workproduct_to_incr_comp_cache_dir, finalize_session_directory,
    in_incr_comp_dir, in_incr_comp_dir_sess, load_query_result_cache, save_work_product_index,
    setup_dep_graph,
};
use rustc_middle::util::Providers;

#[allow(missing_docs)]
pub fn provide(providers: &mut Providers) {
    providers.hooks.save_dep_graph = |tcx| {
        let index_mapper = match tcx.dep_graph.finish_encoding() {
            Ok(mapper) => mapper,
            Err((path, error)) => {
                tcx.sess.dcx().emit_fatal(errors::FailedWritingFile { path: &path, error })
            }
        };

        tcx.sess.time("serialize_dep_graph", || persist::save_dep_graph(tcx, index_mapper))
    };
}

rustc_fluent_macro::fluent_messages! { "../messages.ftl" }
