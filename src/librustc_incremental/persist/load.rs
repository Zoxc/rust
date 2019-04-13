//! Code to save/load the dep-graph from files.

use rustc_data_structures::fx::FxHashMap;
use rustc::dep_graph::{
    LoadResult, PreviousDepGraph, SerializedDepGraph, DepGraphFuture,
    DepGraphData, MaybeAsync,
};
use rustc::session::Session;
use rustc::ty::query::OnDiskCache;
use rustc::util::common::time_ext;
use rustc_serialize::Decodable as RustcDecodable;
use rustc_serialize::opaque::Decoder;
use rustc_serialize::Encodable;
use std::path::Path;

use super::data::*;
use super::fs::*;
use super::file_format;
use super::work_product;
use super::save::save_in;

pub fn open_load_result(
    result: LoadResult<DepGraphData>,
    sess: &Session
) -> DepGraphData {
    match result {
        LoadResult::Error { message } => {
            sess.warn(&message);
            DepGraphData::empty(&temp_dep_graph_path_from(&sess.incr_comp_session_dir()))
        },
        LoadResult::DataOutOfDate => {
            if let Err(err) = delete_all_session_dir_contents(sess) {
                sess.err(&format!("Failed to delete invalidated or incompatible \
                                    incremental compilation session directory contents `{}`: {}.",
                                    dep_graph_path(sess).display(), err));
            }
            DepGraphData::empty(&temp_dep_graph_path_from(&sess.incr_comp_session_dir()))
        }
        LoadResult::Ok { data } => data
    }
}

fn load_data(report_incremental_info: bool, path: &Path) -> LoadResult<(Vec<u8>, usize)> {
    match file_format::read_file(report_incremental_info, path) {
        Ok(Some(data_and_pos)) => LoadResult::Ok {
            data: data_and_pos
        },
        Ok(None) => {
            // The file either didn't exist or was produced by an incompatible
            // compiler version. Neither is an error.
            LoadResult::DataOutOfDate
        }
        Err(err) => {
            LoadResult::Error {
                message: format!("could not load dep-graph from `{}`: {}",
                                  path.display(), err)
            }
        }
    }
}

fn delete_dirty_work_product(sess: &Session,
                             swp: SerializedWorkProduct) {
    debug!("delete_dirty_work_product({:?})", swp);
    work_product::delete_workproduct_files(sess, &swp.work_product);
}

/// Launch a thread and load the dependency graph in the background.
pub fn load_dep_graph(sess: &Session) -> DepGraphFuture {
    // Since `sess` isn't `Sync`, we perform all accesses to `sess`
    // before we fire the background thread.

    let time_passes = sess.time_passes();

    assert!(sess.opts.incremental.is_some());

    // Calling `sess.incr_comp_session_dir()` will panic if `sess.opts.incremental.is_none()`.
    // Fortunately, we just checked that this isn't the case.
    let path = dep_graph_path_from(&sess.incr_comp_session_dir());
    let temp_path = temp_dep_graph_path_from(&sess.incr_comp_session_dir());
    let report_incremental_info = sess.opts.debugging_opts.incremental_info;
    let expected_hash = sess.opts.dep_tracking_hash();

    // Write the file header to the temp file
    save_in(sess, temp_path.clone(), |encoder| {
        // Encode the commandline arguments hash
        sess.opts.dep_tracking_hash().encode(encoder).unwrap();
    });

    let mut prev_work_products = FxHashMap::default();

    // If we are only building with -Zquery-dep-graph but without an actual
    // incr. comp. session directory, we skip this. Otherwise we'd fail
    // when trying to load work products.
    if sess.incr_comp_session_dir_opt().is_some() {
        let work_products_path = work_products_path(sess);
        let load_result = load_data(report_incremental_info, &work_products_path);

        if let LoadResult::Ok { data: (work_products_data, start_pos) } = load_result {
            // Decode the list of work_products
            let mut work_product_decoder = Decoder::new(&work_products_data[..], start_pos);
            let work_products: Vec<SerializedWorkProduct> =
                RustcDecodable::decode(&mut work_product_decoder).unwrap_or_else(|e| {
                    let msg = format!("Error decoding `work-products` from incremental \
                                    compilation session directory: {}", e);
                    sess.fatal(&msg[..])
                });

            for swp in work_products {
                let mut all_files_exist = true;
                for &(_, ref file_name) in swp.work_product.saved_files.iter() {
                    let path = in_incr_comp_dir_sess(sess, file_name);
                    if !path.exists() {
                        all_files_exist = false;

                        if sess.opts.debugging_opts.incremental_info {
                            eprintln!("incremental: could not find file for work \
                                    product: {}", path.display());
                        }
                    }
                }

                if all_files_exist {
                    debug!("reconcile_work_products: all files for {:?} exist", swp);
                    prev_work_products.insert(swp.id, swp.work_product);
                } else {
                    debug!("reconcile_work_products: some file for {:?} does not exist", swp);
                    delete_dirty_work_product(sess, swp);
                }
            }
        }
    }

    MaybeAsync::Async(std::thread::spawn(move || {
        time_ext(time_passes, None, "background load prev dep-graph", move || {
            match load_data(report_incremental_info, &path) {
                LoadResult::DataOutOfDate => LoadResult::DataOutOfDate,
                LoadResult::Error { message } => LoadResult::Error { message },
                LoadResult::Ok { data: (bytes, start_pos) } => {

                    let mut decoder = Decoder::new(&bytes, start_pos);
                    let prev_commandline_args_hash = u64::decode(&mut decoder)
                        .expect("Error reading commandline arg hash from cached dep-graph");

                    if prev_commandline_args_hash != expected_hash {
                        if report_incremental_info {
                            println!("[incremental] completely ignoring cache because of \
                                    differing commandline arguments");
                        }
                        // We can't reuse the cache, purge it.
                        debug!("load_dep_graph_new: differing commandline arg hashes");

                        // No need to do any further work
                        return LoadResult::DataOutOfDate;
                    }

                    let dep_graph = SerializedDepGraph::decode(&mut decoder)
                        .expect("Error reading cached dep-graph");

                    LoadResult::Ok { data: DepGraphData::new(
                        PreviousDepGraph::new(dep_graph),
                        prev_work_products,
                        &temp_path,
                    ) }
                }
            }
        })
    }))
}

pub fn load_query_result_cache<'tcx>(tcx: TyCtxt<'_, 'tcx, 'tcx>) -> OnDiskCache<'tcx> {
    let sess = tcx.sess;
    assert!(sess.opts.incremental.is_some());
    let temp_path = temp_query_cache_path(sess);

    if !sess.opts.debugging_opts.incremental_queries {
        return OnDiskCache::new_empty(tcx, &temp_path);
    }
    match load_data(sess.opts.debugging_opts.incremental_info, &query_cache_path(sess)) {
        LoadResult::Ok{ data: (bytes, start_pos) } => {
            OnDiskCache::new(tcx, bytes, start_pos, &temp_path)
        },
        _ => OnDiskCache::new_empty(tcx, &temp_path)
    }
}
