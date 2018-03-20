/*
use jobs::{job, Symbol, recheck_result_of};
use std::fs;
use std::slice::SliceConcatExt;
use std::ffi::OsStr;
*/
use super::{
    bootstrap_folder, bootstrap_compiler, build_dir, cargo_home, incremental, repo,
    BuiltCompiler, BuiltSysroot, CargoOutput, Compiler, CompilerAndSysroot, PlatformTriple,
    Sysroot,
};
use jobs::{tasks, Builder};
use serde_derive::{Deserialize, Serialize};
use std::io::BufRead;
use std::io::BufReader;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use crate::compile::{CargoMessage, CargoTarget};
use crate::util::{self, is_dylib};
use crate::clean::rm_rf;

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Hash, Debug)]
pub struct CargoConfig {
    pub compiler: Compiler,
    pub sysroot: Option<Sysroot>,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct ReadyCargoCache {
    compiler: BuiltCompiler,
    bootstrap: BuiltCompiler,
    sysroot: Option<BuiltSysroot>,
}

impl ReadyCargoCache {
    fn target_dir(&self) -> PathBuf {
        build_dir()
            .join(&self.compiler.compiler.host.0)
            .join(format!(
                "stage{}-using-{}",
                self.compiler.compiler.stage,
                self.sysroot
                    .map(|sysroot| {
                        if sysroot.sysroot.target == self.compiler.compiler.host {
                            sysroot.sysroot.libs.str().to_string()
                        } else {
                            format!(
                                "{}-{}",
                                sysroot.sysroot.libs.str(),
                                sysroot.sysroot.target.0
                            )
                        }
                    })
                    .unwrap_or("empty".to_string())
            ))
    }

    pub fn build(
        &self,
        builder: &Builder,
        target: PlatformTriple,
        cmd: &str,
        f: impl FnOnce(&mut Command),
    ) -> CargoOutput {
        let sysroot = self
            .sysroot
            .map(|sysroot| sysroot.path())
            .unwrap_or_else(|| build_dir().join("dummy-sysroot"));
        let target_dir = self.target_dir();
        let mut cargo = cargo(
            builder,
            self,
            &sysroot,
            &target_dir,
            target,
            self.compiler.compiler.stage,
            cmd,
        );
        //cargo.arg("-j1");
        f(&mut cargo);
        eprintln!("running cargo: {:?}", cargo);
        run_cargo(&mut cargo, &target_dir, false)
    }
}

fn run_cargo(cargo: &mut Command, out_dir: &Path, is_check: bool) -> CargoOutput {
    let mut child = match cargo.spawn() {
        Ok(child) => child,
        Err(e) => panic!("failed to execute command: {:?}\nerror: {}", cargo, e),
    };

    // `target_root_dir` looks like $dir/$target/release
    let target_root_dir = out_dir;
    // `target_deps_dir` looks like $dir/$target/release/deps
    // FIXME: Do we need this to ignore files outside this?
    let target_deps_dir = target_root_dir.join("deps");
    // `host_root_dir` looks like $dir/release
    let host_root_dir = target_root_dir.parent().unwrap() // chop off `release`
                                       .parent().unwrap() // chop off `$target`
                                       .join(target_root_dir.file_name().unwrap());

    // Spawn Cargo slurping up its JSON output. We'll start building up the
    // `deps` array of all files it generated along with a `toplevel` array of
    // files we need to probe for later.
    let mut deps = Vec::new();
    let ok = stream_cargo(cargo, &mut |msg| {
        let (filenames, crate_types) = match msg {
            CargoMessage::CompilerArtifact {
                filenames,
                target: CargoTarget {
                    crate_types,
                },
                ..
            } => (filenames, crate_types),
            _ => return,
        };
        for filename in filenames {
            // Skip files like executables
            if !filename.ends_with(".rlib") &&
               !filename.ends_with(".lib") &&
               !is_dylib(&filename) &&
               !(is_check && filename.ends_with(".rmeta")) {
                continue;
            }

            let filename = Path::new(&*filename);

            // If this was an output file in the "host dir" we don't actually
            // worry about it, it's not relevant for us
            if filename.starts_with(&host_root_dir) {
                // Unless it's a proc macro used in the compiler
                if !crate_types.iter().any(|t| t == "proc-macro") {
                    continue;
                }
            }

            deps.push((
                filename.to_path_buf(),
                jobs::util::mtime_untracked(filename),
            ));
        }
    });

    if !ok {
        panic!("cargo streaming error");
    }

    // Make sure Cargo actually succeeded after we read all of its stdout.
    let status = t!(child.wait());
    if !status.success() {
        panic!(
            "command did not execute successfully: {:?}\n\
             expected success, got: {}",
            cargo, status
        );
    }

    //deps.iter().for_each(|r| eprintln!("deps {:?}", r));
    deps.sort_by_key(|d| d.0.clone());
    deps
}

pub fn stream_cargo(
    cargo: &mut Command,
    cb: &mut dyn FnMut(CargoMessage<'_>),
) -> bool {
    // Instruct Cargo to give us json messages on stdout, critically leaving
    // stderr as piped so we can get those pretty colors.
    cargo.arg("--message-format").arg("json")
         .stdout(Stdio::piped());

    println!("running: {:?}", cargo);
    let mut child = match cargo.spawn() {
        Ok(child) => child,
        Err(e) => panic!("failed to execute command: {:?}\nerror: {}", cargo, e),
    };

    // Spawn Cargo slurping up its JSON output. We'll start building up the
    // `deps` array of all files it generated along with a `toplevel` array of
    // files we need to probe for later.
    let stdout = BufReader::new(child.stdout.take().unwrap());
    for line in stdout.lines() {
        let line = t!(line);
        match serde_json::from_str::<CargoMessage<'_>>(&line) {
            Ok(msg) => cb(msg),
            // If this was informational, just print it out and continue
            Err(_) => println!("{}", line)
        }
    }

    // Make sure Cargo actually succeeded after we read all of its stdout.
    let status = t!(child.wait());
    if !status.success() {
        eprintln!("command did not execute successfully: {:?}\n\
                  expected success, got: {}",
                 cargo,
                 status);
    }
    status.success()
}

fn cargo(
    builder: &Builder,
    cache: &ReadyCargoCache,
    sysroot: &Path,
    out_dir: &Path,
    target: PlatformTriple,
    stage: u32,
    cmd: &str,
) -> Command {
    let mut cargo = Command::new(&bootstrap_folder().join("bin").join("cargo"));
    cargo.current_dir(repo().join("src"));
    //println!("cargo dir {:?}", repo().join("src"));
    cargo
        .env("CARGO_TARGET_DIR", out_dir)
        .env("CARGO_HOME", cargo_home())
        .arg(cmd)
        .arg("--target")
        .arg(&target.0);

    // See comment in librustc_llvm/build.rs for why this is necessary, largely llvm-config
    // needs to not accidentally link to libLLVM in stage0/lib.
    cargo.env("REAL_LIBRARY_PATH_VAR", &util::dylib_path_var());
    if let Some(e) = env::var_os(util::dylib_path_var()) {
        cargo.env("REAL_LIBRARY_PATH", e);
    }

    let mut rustflags: Vec<String> = Vec::new();

    rustflags.push(format!("--cfg stage{}", stage));
    rustflags.push("-Z force-unstable-if-unmarked".to_string());

    rustflags.push("-Z share-generics=off".to_string());

    cargo.env("RUSTC_FORCE_UNSTABLE", "1");
    cargo.env("RUSTC_VERBOSE", "2");

    cargo.env("RUSTFLAGS", rustflags.join(" "));
    cargo.env("RUSTC_CODEGEN_UNITS", "1");
    //cargo.env("RUSTC_DEBUGINFO", "true");

    // FIXME: Temporary fix for https://github.com/rust-lang/cargo/issues/3005
    // Force cargo to output binaries with disambiguating hashes in the name
    //cargo.env("__CARGO_DEFAULT_LIB_METADATA", &self.config.channel);

    // Customize the compiler we're running. Specify the compiler to cargo
    // as our shim and then pass it some various options used to configure
    // how the actual compiler itself is called.
    //
    // These variables are primarily all read by
    // src/bootstrap/bin/{rustc.rs,rustdoc.rs}
    cargo
        .env("RUSTC", repo().join(path!("build", "bootstrap", "debug", "rustc")))
        .env("RUSTC_REAL", cache.compiler.bin())
        .env("RUSTC_STAGE", stage.to_string())
        .env("RUSTC_SYSROOT", sysroot)
        .env("RUSTC_LIBDIR", cache.compiler.libdir());
    // .env("RUSTC_RPATH", self.config.rust_rpath.to_string())

    // Enable usage of unstable features
    cargo.env("RUSTC_BOOTSTRAP", "1");

    cargo.env("RUSTC_SNAPSHOT", cache.bootstrap.bin())
         .env("RUSTC_SNAPSHOT_LIBDIR", cache.bootstrap.libdir());

    // Ignore incremental modes except for stage0, since we're
    // not guaranteeing correctness across builds if the compiler
    // is changing under your feet.`
    let incremental = incremental(); // && compiler.stage == 0;
    cargo.env("CARGO_INCREMENTAL", format!("{}", incremental as usize));

    // For `cargo doc` invocations, make rustdoc print the Rust version into the docs
    //cargo.env("RUSTDOC_CRATE_VERSION", self.build.rust_version());

    // Environment variables *required* throughout the build
    //
    // FIXME: should update code to not require this env var
    cargo.env("CFG_COMPILER_HOST_TRIPLE", target.0);

    // Set this for all builds to make sure doc builds also get it.
    cargo.env("CFG_RELEASE_CHANNEL", "nightly");

    //if mode != Mode::Tool {
    cargo.env("WINAPI_NO_BUNDLED_LIBRARIES", "1");
    //}

    //if let Some(x) = self.crt_static(target) {
        cargo.env("RUSTC_CRT_STATIC", true.to_string());
    //}

    //if let Some(true) = self.crt_static(compiler.host) {
        cargo.env("RUSTC_HOST_CRT_STATIC", true.to_string());
    //}

    //for _ in 1..self.verbosity {
    //cargo.arg("-v");
    //}

    cargo
}

tasks! {
    builder_var: builder;
    pub group TASKS;

    #[no_early_cutoff]
    pub task CargoCache {
        compiler: Compiler,
        sysroot: Option<Sysroot>,
    } -> ReadyCargoCache {
        let ready = ReadyCargoCache {
            compiler: builder.run(compiler),
            bootstrap: builder.run(bootstrap_compiler()),
            sysroot: sysroot.map(|sysroot| builder.run(CompilerAndSysroot {
                compiler: compiler,
                sysroot: sysroot,
            }).sysroot ),
        };
        rm_rf(&ready.target_dir());
        ready
    }
}
