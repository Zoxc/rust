// Copyright 2012-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use super::*;
use common::{CombineTest, Config, TestPaths};
use common::RunPass;
use header::TestProps;
use itertools::Itertools;

use test::{TestFn, TestDescAndFn, TestResult, TestEvent};

use std::collections::HashSet;
use std::panic;
use std::fs::{File, OpenOptions};
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use std::io::SeekFrom;
use std::io::Seek;
use std::time::SystemTime;
use std::path::{PathBuf};

pub fn run_combined(config: Arc<Config>, all_tests: Vec<TestDescAndFn>,
                    _threads: usize) -> Option<JoinHandle<Vec<TestEvent>>> {
    if config.mode != RunPass {
        assert!(all_tests.is_empty());
        return None;
    }
    Some(thread::spawn(move || {
        let threads = 16;
        let n = (all_tests.len() / threads) + 1;
        let threads = all_tests.into_iter().chunks(n);
        let threads: Vec<_> = threads.into_iter().enumerate().map(|(i, tests)| {
            let tests: Vec<_> = tests.collect();
            let config = config.clone();
            thread::spawn(move || {
                run_combined_instance(&*config, i * 10000, tests)
            })
        }).collect();
        threads.into_iter().flat_map(|thread| {
            thread.join().unwrap().0
        }).collect()
    }))
}

pub fn run_combined_instance(config: &Config,
                    instance: usize,
                    all_tests: Vec<TestDescAndFn>) -> (Vec<TestEvent>, Vec<PathBuf>, u64) {
    let mut events = Vec::new();
    //println!("running {} combined tests", all_tests.len());
    let mut tests: Vec<_> = Vec::new();

    for mut test in all_tests {
        let r = match test.testfn {
            TestFn::CombineTest(ref mut t) => {
                let mut combine = t.downcast_mut::<CombineTest>().unwrap();

                if test.desc.ignore {
                    None
                } else {
                    Some(combine.paths.file.clone())
                }
            }
            _ => panic!(),
        };
        match r {
            Some(r) => tests.push((test, r)),
            None => {
                events.push(TestEvent::TeResult(test.desc, TestResult::TrIgnored, Vec::new()));
            }
        }
    }

    let file = config.build_base.join(format!("run-pass-{}.rs", instance));
    let progress_file = config.build_base.join(format!("run-pass-progress-{}", instance));

    let mut input = File::create(&file).unwrap();

    let mut out = String::new();
/*
    out.push_str("#![feature(cfg_target_vendor, cfg_target_feature, concat_idents)]
#![feature(asm, global_asm, trace_macros, log_syntax, macro_vis_matcher, no_core, fn_traits)]
#![feature(macro_at_most_once_rep, macro_lifetime_matcher, mpsc_select, slice_patterns)]
#![feature(box_syntax, slice_patterns, custom_attribute, raw)]
#![feature(non_ascii_idents, i128_type, box_patterns, const_fn, underscore_lifetimes)]
#![feature(unboxed_closures, type_ascription, optin_builtin_traits, specialization)]
#![feature(global_allocator, specialization, target_feature, repr_simd, platform_intrinsics)]
#![feature(crate_in_paths, if_while_or_patterns, intrinsics, pattern_parentheses, used)]
#![feature(placement_in_syntax, pattern_parentheses, exclusive_range_pattern, decl_macro)]
#![feature(crate_visibility_modifier, non_exhaustive, rustc_attrs, generic_param_attrs)]
#![feature(dropck_parametricity, std_misc, core_intrinsics, catch_expr, abi_vectorcall)]
#![feature(associated_type_defaults, link_llvm_intrinsics, generators, extern_types)]
#![feature(abi_thiscall, dyn_trait, dropck_eyepatch, unwind_attributes, generator_trait)]
#![feature(untagged_unions, arbitrary_self_types, try_trait, lookup_host, clone_closures)]
#![feature(process_exitcode_placeholder, unsized_tuple_coercion, match_default_bindings)]
#![feature(conservative_impl_trait, universal_impl_trait, const_type_id, copy_closures)]
#![feature(link_args, linkage)]
*/
    out.push_str("#![allow(warnings)]
//extern crate core;
");

    for (i, test) in tests.iter().enumerate() {
        out.push_str("#[path=\"");
        out.push_str(&test.1.to_str().unwrap().to_string().replace("\\", "\\\\"));
        out.push_str(&format!("\"]\nmod _combined_test_{};\n", i));
    }

    let mut progress = File::create(&progress_file).unwrap();
    progress.write_all("0".as_bytes()).unwrap();
    progress.flush().unwrap();
    drop(progress);

    out.push_str("fn main() {
        use std::fs::OpenOptions;
        use std::io::Read;
        use std::io::SeekFrom;
        use std::io::Seek;
        use std::io::Write;
        println!(\"{:?}\", std::env::current_dir());
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
    ");
    out.push_str(&format!(".open(\"run-pass-progress-{}\").unwrap();", instance));
    out.push_str("
        let mut c = String::new();
        file.read_to_string(&mut c).unwrap();
        let mut i = c.parse::<usize>().unwrap();
        loop {
            file.seek(SeekFrom::Start(0)).unwrap();
            file.set_len(0).unwrap();
            file.write_all(format!(\"{}\", i).as_bytes()).unwrap();
            file.flush().unwrap();
            match i {
    ");

    for (i, test) in tests.iter().enumerate() {
        out.push_str(&format!("    {} => ", i));
        out.push_str("{\n    println!(\"Running ");
        out.push_str(&test.1.to_str().unwrap().to_string().replace("\\", "\\\\"));
        out.push_str("\");\n");
        out.push_str(&format!("    _combined_test_{}::main();\n}}\n", i));
    }

    out.push_str(" _ => break }
            i += 1;
        }
    }\n");

    input.write_all(out.as_bytes()).unwrap();
    input.flush().unwrap();

    let paths = TestPaths {
        file: file,
        base: config.src_base.clone(),
        relative_dir: PathBuf::from("."),
    };

    let mut props = TestProps::new();
    props.compile_flags.push("-C".to_string());
    props.compile_flags.push("codegen-units=1".to_string());
    props.compile_flags.push("-A".to_string());
    props.compile_flags.push("warnings".to_string());
    props.compile_flags.push("-Z".to_string());
    props.compile_flags.push("submodules-crate-like".to_string());

    let base_cx = TestCx {
        config: &config,
        props: &props,
        testpaths: &paths,
        revision: None,
        long_compile: true,
    };

    //let start = SystemTime::now();

    base_cx.compile_rpass_test();

    //let time = SystemTime::now().duration_since(start).unwrap();
    //println!("run-pass combined test {} compiled in {} seconds", instance, time.as_secs());

    let mut failed = HashSet::new();
    //let start = SystemTime::now();

    loop {
        match panic::catch_unwind(|| {
            let proc_res = base_cx.exec_compiled_test();
            if !proc_res.status.success() {
                base_cx.fatal_proc_rec("test run failed!", &proc_res);
            }
        }) {
            Err(_) => {
                // Skip the offending test and try again
                let mut file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&progress_file).unwrap();
                let mut c = String::new();
                file.read_to_string(&mut c).expect("unable to read test progress");
                let mut i = c.parse::<usize>().unwrap();
                failed.insert(i);
                println!("Panic during {:?} ({})", tests[i].1, i);
                if i >= tests.len() {
                    panic!("run-pass test stuck in a loop")
                } else {
                    i += 1;
                }
                file.seek(SeekFrom::Start(0)).unwrap();
                file.set_len(0).unwrap();
                file.write_all(format!("{}", i).as_bytes()).unwrap();
                file.flush().unwrap();
            }
            Ok(()) => break,
        }
    }

    //let time = SystemTime::now().duration_since(start).unwrap();
    //println!("run-pass combined test {} ran in {} seconds", instance, time.as_secs());

    let mut paths = Vec::new();

    for (i, test) in tests.into_iter().enumerate() {
        let result = if failed.contains(&i) {
            TestResult::TrFailed
        } else {
            TestResult::TrOk
        };
        paths.push(test.1);
        events.push(TestEvent::TeResult(test.0.desc, result, Vec::new()));
    }

    (events, paths, 0/*time.as_secs()*/)
}
