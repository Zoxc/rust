use super::{build_dir, host_triple, repo, GitVersion, PlatformTriple};
use cmake;
use jobs::{tasks, Symbol};
use std::env;
use std::ffi::{OsString};
use std::fs;
use std::path::{Path, PathBuf};
//use crate::clean::rm_rf;
use crate::alt_build::util_2::timeit;
use crate::util::exe;

fn llvm_out_dir() -> PathBuf {
    build_dir().join(host_triple().0).join("llvm")
}

fn llvm_repo() -> PathBuf {
    repo().join(path!("src", "llvm-project", "llvm"))
}

tasks! {
    builder_var: builder;
    pub group TASKS;

    #[no_early_cutoff]
    pub task LLVMCache {
        target: PlatformTriple,
    } -> () {
        let v = builder.run(GitVersion { path: llvm_repo() });
        println!("LLVM rev: {:?}", v);
        // FIXME: Useful to keep this for debugging
        //util::rm_rf(&llvm_out_dir());
    }

    #[no_early_cutoff]
    pub task LLVM {
        target: PlatformTriple,
    } -> PathBuf {
        let v = builder.run(LLVMCache { target });
        run(target.0)
    }
}

/// Compile LLVM for `target`.
fn run(target: Symbol) -> PathBuf {
    let (out_dir, llvm_config_ret_dir) = {
        let mut dir = llvm_out_dir();
        (dir.clone(), dir.join("bin"))
    };
    let build_llvm_config = llvm_out_dir().join("bin").join(exe("llvm-config", &*target));

    let descriptor = "";
    println!("Building {}LLVM for {}", descriptor, target);
    let _time = timeit();
    t!(fs::create_dir_all(&out_dir));

    // http://llvm.org/docs/CMake.html
    let mut cfg = cmake::Config::new(llvm_repo());

    let profile = match (true, false) {
        (false, _) => "Debug",
        (true, false) => "Release",
        (true, true) => "RelWithDebInfo",
    };

    // NOTE: remember to also update `config.toml.example` when changing the
    // defaults!
    let llvm_targets = "X86";

    let llvm_exp_targets = "";

    let assertions = if false {"ON"} else {"OFF"};

    cfg.out_dir(&out_dir)
        .profile(profile)
        .define("LLVM_ENABLE_ASSERTIONS", assertions)
        .define("LLVM_TARGETS_TO_BUILD", llvm_targets)
        .define("LLVM_EXPERIMENTAL_TARGETS_TO_BUILD", llvm_exp_targets)
        .define("LLVM_INCLUDE_EXAMPLES", "OFF")
        .define("LLVM_INCLUDE_TESTS", "OFF")
        .define("LLVM_INCLUDE_DOCS", "OFF")
        .define("LLVM_ENABLE_ZLIB", "OFF")
        .define("WITH_POLLY", "OFF")
        .define("LLVM_ENABLE_TERMINFO", "OFF")
        .define("LLVM_ENABLE_LIBEDIT", "OFF")
        .define("LLVM_PARALLEL_COMPILE_JOBS", "8")
        .define("LLVM_TARGET_ARCH", target.split('-').next().unwrap())
        .define("LLVM_DEFAULT_TARGET_TRIPLE", target);

    // By default, LLVM will automatically find OCaml and, if it finds it,
    // install the LLVM bindings in LLVM_OCAML_INSTALL_PATH, which defaults
    // to /usr/bin/ocaml.
    // This causes problem for non-root builds of Rust. Side-step the issue
    // by setting LLVM_OCAML_INSTALL_PATH to a relative path, so it installs
    // in the prefix.
    cfg.define("LLVM_OCAML_INSTALL_PATH",
        env::var_os("LLVM_OCAML_INSTALL_PATH").unwrap_or_else(|| "usr/lib/ocaml".into()));

    // This setting makes the LLVM tools link to the dynamic LLVM library,
    // which saves both memory during parallel links and overall disk space
    // for the tools.  We don't distribute any of those tools, so this is
    // just a local concern.  However, it doesn't work well everywhere.
    if target.contains("linux-gnu") || target.contains("apple-darwin") {
        cfg.define("LLVM_LINK_LLVM_DYLIB", "ON");
    }

    if target.contains("msvc") {
        cfg.define("LLVM_USE_CRT_DEBUG", "MT");
        cfg.define("LLVM_USE_CRT_RELEASE", "MT");
        cfg.define("LLVM_USE_CRT_RELWITHDEBINFO", "MT");
        cfg.static_crt(true);
    }

    if target.starts_with("i686") {
        cfg.define("LLVM_BUILD_32_BITS", "ON");
    }

    if let Some(num_linkers) = Some(2) {
        if num_linkers > 0 {
            cfg.define("LLVM_PARALLEL_LINK_JOBS", num_linkers.to_string());
        }
    }
/*
    // http://llvm.org/docs/HowToCrossCompileLLVM.html
    if target != build.build && !emscripten {
        builder.ensure(Llvm {
            target: build.build,
            emscripten: false,
        });
        // FIXME: if the llvm root for the build triple is overridden then we
        //        should use llvm-tblgen from there, also should verify that it
        //        actually exists most of the time in normal installs of LLVM.
        let host = build.llvm_out(build.build).join("bin/llvm-tblgen");
        cfg.define("CMAKE_CROSSCOMPILING", "True")
            .define("LLVM_TABLEGEN", &host);

        if target.contains("netbsd") {
            cfg.define("CMAKE_SYSTEM_NAME", "NetBSD");
        } else if target.contains("freebsd") {
            cfg.define("CMAKE_SYSTEM_NAME", "FreeBSD");
        }

        cfg.define("LLVM_NATIVE_BUILD", build.llvm_out(build.build).join("build"));
    }
*/
    configure_cmake(target, &mut cfg, false);

    // FIXME: we don't actually need to build all LLVM tools and all LLVM
    //        libraries here, e.g. we just want a few components and a few
    //        tools. Figure out how to filter them down and only build the right
    //        tools and libs on all platforms.
    cfg.build();

    build_llvm_config
}

fn configure_cmake(target: Symbol,
                   cfg: &mut cmake::Config,
                   building_dist_binaries: bool) {
    cfg.target(&*target)
       .host(&*host_triple().0);

    let sanitize_cc = |cc: &Path| {
        if target.contains("msvc") {
            OsString::from(cc.to_str().unwrap().replace("\\", "/"))
        } else {
            cc.as_os_str().to_owned()
        }
    };
/*
    // MSVC with CMake uses msbuild by default which doesn't respect these
    // vars that we'd otherwise configure. In that case we just skip this
    // entirely.
    if target.contains("msvc") && !build.config.ninja {
        return
    }

    let cc = build.cc(target);
    let cxx = build.cxx(target).unwrap();

    // Handle msvc + ninja + ccache specially (this is what the bots use)
    if target.contains("msvc") &&
       build.config.ninja &&
       build.config.ccache.is_some() {
        let mut cc = env::current_exe().expect("failed to get cwd");
        cc.set_file_name("sccache-plus-cl.exe");

       cfg.define("CMAKE_C_COMPILER", sanitize_cc(&cc))
          .define("CMAKE_CXX_COMPILER", sanitize_cc(&cc));
       cfg.env("SCCACHE_PATH",
               build.config.ccache.as_ref().unwrap())
          .env("SCCACHE_TARGET", target);

    // If ccache is configured we inform the build a little differently hwo
    // to invoke ccache while also invoking our compilers.
    } else if let Some(ref ccache) = build.config.ccache {
       cfg.define("CMAKE_C_COMPILER", ccache)
          .define("CMAKE_C_COMPILER_ARG1", sanitize_cc(cc))
          .define("CMAKE_CXX_COMPILER", ccache)
          .define("CMAKE_CXX_COMPILER_ARG1", sanitize_cc(cxx));
    } else {
       cfg.define("CMAKE_C_COMPILER", sanitize_cc(cc))
          .define("CMAKE_CXX_COMPILER", sanitize_cc(cxx));
    }

    cfg.build_arg("-j").build_arg(build.jobs().to_string());
    cfg.define("CMAKE_C_FLAGS", build.cflags(target).join(" "));
    let mut cxxflags = build.cflags(target).join(" ");
    if building_dist_binaries {
        if build.config.llvm_static_stdcpp && !target.contains("windows") {
            cxxflags.push_str(" -static-libstdc++");
        }
    }
    cfg.define("CMAKE_CXX_FLAGS", cxxflags);
    if let Some(ar) = build.ar(target) {
        if ar.is_absolute() {
            // LLVM build breaks if `CMAKE_AR` is a relative path, for some reason it
            // tries to resolve this path in the LLVM build directory.
            cfg.define("CMAKE_AR", sanitize_cc(ar));
        }
    }

    if env::var_os("SCCACHE_ERROR_LOG").is_some() {
        cfg.env("RUST_LOG", "sccache=warn");
    }*/
}
