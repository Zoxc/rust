/*
use jobs::{job, Symbol, recheck_result_of};
use std::fs;
use std::process::Command;
use std::slice::SliceConcatExt;
use std::process::Stdio;
use std::io::BufReader;
use std::io::BufRead;
use std::ffi::OsStr;
*/

//#![allow(warnings)]
use jobs::{tasks, Builder, Symbol};
use serde_derive::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::path::{self, Path, PathBuf};
use std::time::SystemTime;
use std::fs;
use crate::Config;
use crate::util::libdir;
use crate::clean::rm_rf;

macro_rules! path {
    ($($c:expr),*) => ({
        let mut p = PathBuf::new();
        $(p.push($c);)*
        p
    })
}

#[macro_use]
#[path = "util.rs"]
mod util_2;
mod cargo;
mod llvm;

use util_2::{cp_r, copy};

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Copy, Hash, Debug)]
pub struct PlatformTriple(Symbol);

impl Display for PlatformTriple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Copy for Compiler {}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Copy, Debug)]
pub struct BuiltCompiler {
    compiler: Compiler,
}

impl BuiltCompiler {
    fn bin(&self) -> PathBuf {
        if self.compiler.stage == 0 {
            assert_eq!(self.compiler.host, host_triple());
            bootstrap_folder().join("bin").join("rustc")
        } else {
            rustc_sysroot(&self.compiler, None)
                .join("bin")
                .join("rustc")
        }
    }

    pub fn libdir(&self) -> PathBuf {
        if self.compiler.stage == 0 {
            bootstrap_folder().join(libdir(&self.compiler.host.0))
        } else {
            panic!()
        }
    }
}

fn out_dir(target: &Compiler, post_fix: &str) -> PathBuf {
    build_dir()
        .join(&target.host.0)
        .join(format!("stage{}-{}", target.stage, post_fix))
}

fn rustc_sysroot(compiler: &Compiler, post_fix: Option<&str>) -> PathBuf {
    out_dir(
        compiler,
        &format!(
            "sysroot{}",
            post_fix
                .map(|p| format!("-{}", p))
                .unwrap_or("".to_string())
        ),
    )
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Copy, Clone, Hash, Debug)]
enum Libs {
    Std,
    Rustc,
    CodegenLLVM,
}

impl Libs {
    fn str(&self) -> &'static str {
        match self {
            Libs::Std => "std",
            Libs::Rustc => "rustc",
            Libs::CodegenLLVM => "codegen-llvm",
        }
    }
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Copy, Hash, Debug)]
pub struct Sysroot {
    target: PlatformTriple,
    libs: Libs,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Copy, Debug)]
pub struct BuiltSysroot {
    compiler: Compiler,
    sysroot: Sysroot,
}

impl BuiltSysroot {
    fn path(&self) -> PathBuf {
        if self.compiler.stage == 0 && self.sysroot.libs == Libs::Std {
            return build_dir().join(&host_triple().0).join("bootstrap-sysroot");
        }
        if self.sysroot.target == self.compiler.host {
            build_dir().join(&self.compiler.host.0).join(format!(
                "stage{}-sysroot-with-{}",
                self.compiler.stage,
                self.sysroot.libs.str()
            ))
        } else {
            build_dir().join(&self.compiler.host.0).join(format!(
                "stage{}-sysroot-with-{}-{}",
                self.compiler.stage,
                self.sysroot.libs.str(),
                self.sysroot.target.0
            ))
        }
    }
}

impl Compiler {
    fn prev_stage(&self) -> Compiler {
        assert!(self.stage > 0);
        Compiler {
            stage: self.stage - 1,
            host: host_triple(),
        }
    }
}

impl Display for Compiler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stage{}.{}", self.stage, self.host)
    }
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct BuiltCompilerAndSysroot {
    compiler: BuiltCompiler,
    sysroot: BuiltSysroot,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct BuiltLibs {
    compiler: BuiltCompiler,
    output: CargoOutput,
}

type CargoOutput = Vec<(PathBuf, SystemTime)>;

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct BuiltStdLibs {
    compiler: Compiler,
    target: Symbol,
    output: CargoOutput,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct BuiltCompilerLibs {
    target_compiler: Compiler,
    output: CargoOutput,
}

fn cargo_home() -> PathBuf {
    build_dir().join("registry")
}

fn host_triple() -> PlatformTriple {
    PlatformTriple(Symbol::new("x86_64-pc-windows-msvc"))
}

fn repo() -> PathBuf {
    canonicalize(&absolute("."))
}

fn bootstrap_compiler() -> Compiler {
    Compiler {
        stage: 0,
        host: host_triple(),
    }
}

fn incremental() -> bool {
    false
}

fn bootstrap_folder() -> PathBuf {
    repo().join(path!("build", "base", host_triple().0))
}

fn canonicalize<P: AsRef<Path>>(path: P) -> PathBuf {
    let p = path.as_ref().canonicalize().unwrap();
    #[cfg(windows)]
    {
        let mut components = p.components();
        let new_root = match components.next().unwrap() {
            path::Component::Prefix(prefix_component) => match prefix_component.kind() {
                path::Prefix::VerbatimDisk(d) => Some(d as char),
                _ => None,
            },
            _ => None,
        };
        if let Some(drive) = new_root {
            return Path::new(&format!("{}:", drive)).join(components.as_path());
        }
    }
    p
}

fn absolute<P: AsRef<Path>>(path: P) -> PathBuf {
    std::env::current_dir().unwrap().join(path)
}

fn build_dir() -> PathBuf {
    absolute("output")
}

/*
#[macro_use]
mod util;

mod llvm;

#[job]
fn rustc_bin(compiler: Compiler) -> PathBuf {
    if compiler.stage == 0 {
        assert_eq!(compiler.host, host());
        bootstrap_bin_folder().join("rustc")
    } else {
        assemble_rustc(compiler);
        rustc_sysroot(compiler, None).join("bin").join("rustc")
    }
}

fn rustc_sysroot(compiler: Compiler, post_fix: Option<&str>) -> PathBuf {
    out_dir(compiler, &format!("sysroot{}", post_fix.map(|p| format!("-{}", p)).unwrap_or("".to_string())))
}

fn rustc_sysroot_lib_dir(compiler: Compiler, target: Symbol, post_fix: Option<&str>) -> PathBuf {
    let sysroot = rustc_sysroot(compiler, post_fix);
    sysroot.join("lib").join("rustlib").join(target).join("lib")
}

fn out_dir(target: Compiler, post_fix: &str) -> PathBuf {
    build_dir().join(&target.host).join(format!("stage{}-{}", target.stage, post_fix))
}

fn bootstrap_src() -> PathBuf {
    canonicalize("../rustc-beta-src")
}

#[job(input)]
fn std_libs(compiler: Compiler, target: Symbol, repo: PathBuf) -> CargoOutput {
    println!("rustc_path = {:?}", rustc_bin(compiler));
    println!("Building std libs for {} using {}", target, compiler);
    let cargo_out_dir = out_dir(compiler, "std");
    let stage = if repo.starts_with(bootstrap_src()) {
        2
    } else {
        compiler.stage
    };
    let mut cargo = cargo(compiler, &out_dir(compiler, "fake-sysroot"), &cargo_out_dir, target, stage, "build");
    //cargo.arg("--message-format").arg("json");
   // cargo.arg("-j1");
    cargo.arg("--release");
    //cargo.arg("-vv");
    cargo.arg("--manifest-path").arg(repo.join(path!("src", "libstd", "Cargo.toml")));
    println!("cargo || {:?}", cargo);
    let output = run_cargo(&mut cargo, &cargo_out_dir.join(path!(host(), "release")), false);

    //cargo.status().unwrap();
    output
}
/*
#[job]
fn assemble_bootstrap_std_sysroot() {
    let mut libs = bootstrap_std_libs();
    let lib_dir = rustc_sysroot_lib_dir(bootstrap_compiler(), host(), Some("std"));

    util::create_dir_all(&lib_dir);

    for (lib, _) in libs {
        if lib.extension() == Some(OsStr::new("rlib")) {
            util::copy(&lib, &lib_dir.join(lib.file_name().unwrap()));
        }
    }
}

#[job(input)]
fn test_libs(compiler: Compiler, target: Symbol, repo: PathBuf) -> CargoOutput {
    assemble_bootstrap_std_sysroot();
    println!("Building test libs for {} using {}", target, compiler);
    let cargo_out_dir = out_dir(compiler, "std");
    let sysroot = rustc_sysroot(compiler, Some("std"));
    let mut cargo = cargo(compiler, &sysroot, &cargo_out_dir, target, "build");
    //cargo.arg("--message-format").arg("json");
    cargo.arg("-j1");
    cargo.arg("--manifest-path").arg(repo.join(path!("src", "libtest", "Cargo.toml")));
    let output = run_cargo(&mut cargo, &cargo_out_dir.join(path!(host(), "debug")), false);

    println!("cargo {:?}", cargo);
    //cargo.status().unwrap();
    output
}
*/
#[job]
fn assemble_std(compiler: Compiler, target: Symbol) {
    println!("Assembling std libs for {} using {}", target, compiler);
    let libs = if compiler == bootstrap_compiler() {
        std_libs(compiler, target, bootstrap_src())
    } else {
        std_libs(compiler, target, repo())
    };
    //libs.extend(test_libs(bootstrap_compiler(), host(), bootstrap_src()));

    let lib_dir = rustc_sysroot_lib_dir(compiler, host(), None);

    util::create_dir_all(&lib_dir);

    for (lib, _) in libs {
        if lib.extension() == Some(OsStr::new("rlib")) {
            util::copy(&lib, &lib_dir.join(lib.file_name().unwrap()));
        }
    }
}
/*
#[job(input)]
fn rustc_trans(target: Compiler) -> CargoOutput {
    let host = target.prev_stage();
    assemble_bootstrap_sysroot();
    let llvm_config = llvm::run(target.host);
    println!("llvm = {:?}", llvm_config);
    println!("rustc_path = {:?}", rustc_bin(host));

    println!("Building rustc trans for {} using {}", target, host);
    let cargo_out_dir = out_dir(target, "rustc");
    println!("cargo_out_dir = {:?}", cargo_out_dir);
    let mut cargo = cargo(host, &rustc_sysroot(host, None), &cargo_out_dir, target.host, "build");
    //cargo.arg("--message-format").arg("json");
    cargo.arg("-j1");
    //cargo.arg("-vv");
    cargo.arg("--manifest-path").arg(repo().join(path!("src", "librustc_trans", "Cargo.toml")));
    cargo.env("LLVM_CONFIG", &llvm_config);
    cargo.env("RUSTC_ERROR_METADATA_DST", &out_dir(target, "rustc-trans-errors"));
    let output = run_cargo(&mut cargo, &cargo_out_dir.join(path!(target.host, "debug")), false);

    println!("cargo {:?}", cargo);
    //cargo.status().unwrap();
    output
}
*/
#[job(input)]
fn rustc_libs(target: Compiler) -> CargoOutput {
    let host = target.prev_stage();
    assemble_std(host, target.host);
    let llvm_config = llvm::run(target.host);
    println!("rustc_path = {:?}", rustc_bin(host));
    println!("Building rustc libs for {} using {}", target, host);
    let cargo_out_dir = out_dir(target, "rustc");
    let mut cargo = cargo(host, &rustc_sysroot(host, None), &cargo_out_dir, target.host, host.stage, "rustc");
    //cargo.arg("--message-format").arg("json");
    //cargo.arg("-j1");
    cargo.arg("--release");
    cargo.arg("--manifest-path").arg(repo().join(path!("src", "rustc", "Cargo.toml")));
    cargo.env("LLVM_CONFIG", &llvm_config);
    cargo.env("CFG_VERSION", "1.99.0-nightly");
    cargo.env("RUSTC_ERROR_METADATA_DST", &out_dir(target, "rustc-errors"));
    cargo.arg("--");
    cargo.arg("-Clto");
    //cargo.arg("-Zprint-link-args");
    cargo.arg(&format!("--emit=llvm-bc={}", absolute("red.bc").to_str().unwrap()));
    let output = run_cargo(&mut cargo, &cargo_out_dir.join(path!(target.host, "release")), false);

    println!("cargo {:?}", cargo);
    //cargo.status().unwrap();
    output
}

fn copy_libs_from_bootstrap(lib_dir: &Path, libs: &[&str]) {
    // Copy dynamic libraries from the bootstrap compiler
    let base_libdir = bootstrap_bin_folder().parent().unwrap()
                                .join("lib").join("rustlib").join(host()).join("lib");

    for f in t!(fs::read_dir(&base_libdir)).map(|f| t!(f)) {
        let filename = f.file_name().into_string().unwrap();
        let should_copy = util::is_dylib(&filename) && libs.iter().any(|lib| {
            filename.starts_with(&format!("{}-", lib)) ||
            filename.starts_with(&format!("lib{}-", lib))
        });
        if !should_copy {
            continue;
        }
        let dest = &lib_dir.join(filename);
        util::copy(&f.path(), &dest);
    }
}

#[job]
fn assemble_rustc(target: Compiler) {
    println!("Assembling rustc libs for {}", target);
    let root = rustc_sysroot(target, None);

    let mut libs = rustc_libs(target);
    //libs.extend(rustc_trans(target));

    let lib_dir = root.join(util::libdir(&target.host));
    let bin_dir = root.join("bin");

    util::create_dir_all(&bin_dir);
    util::create_dir_all(&lib_dir);

    if target.stage == 1 {
        copy_libs_from_bootstrap(&lib_dir, &["test", "term", "std"]);
    }

    for (lib, _) in libs {
        if util::is_dylib(lib.to_str().unwrap()) {
            util::copy(&lib, &lib_dir.join(lib.file_name().unwrap()));
        } else if util::is_exe(&lib) {
            util::copy(&lib, &bin_dir.join(lib.file_name().unwrap()));
        }
    }
}
*/

fn build_libs(builder: &Builder, compiler: Compiler, sysroot: Sysroot) -> BuiltLibs {
    let llvm_config = 
            builder.run(llvm::LLVM {
                target: sysroot.target,
            });
    let required = match sysroot.libs {
        Libs::Std => None,
        Libs::Rustc => {
            builder.run(CompilerAndSysroot {
                compiler,
                sysroot: Sysroot {
                    libs: Libs::Std,
                    target: sysroot.target,
                },
            });
            Some(Sysroot {
                target: sysroot.target,
                libs: Libs::Std,
            })
        }
        Libs::CodegenLLVM => {
            Some(Sysroot {
                target: sysroot.target,
                libs: Libs::Rustc,
            })
        }
    };
    let cargo = builder.run(cargo::CargoCache {
        compiler,
        sysroot: required,
    });
    let output = cargo.build(builder, sysroot.target, "build", |cmd| {
        let manifest = match sysroot.libs {
            Libs::Std => repo().join(path!("src", "libtest", "Cargo.toml")),
            Libs::Rustc => {
                if compiler.stage == 0 {
                    cmd.arg("--features").arg("bootstrap_std");
                }
                repo().join(path!("src", "rustc", "Cargo.toml"))
            },
            Libs::CodegenLLVM => repo().join(path!("src", "librustc_codegen_llvm", "Cargo.toml")),
        };
        cmd.arg("--manifest-path").arg(manifest);
        match sysroot.libs {
            Libs::Std => {}
            Libs::Rustc | Libs::CodegenLLVM => {
                cmd.env("LLVM_CONFIG", &llvm_config);
                cmd.env("CFG_VERSION", "1.99.0-nightly");
                cmd.env(
                    "RUSTC_ERROR_METADATA_DST",
                    &out_dir(
                        &Compiler {
                            stage: compiler.stage + 1,
                            host: sysroot.target,
                        },
                        "rustc-errors",
                    ),
                );
            }
        }
    });
    BuiltLibs {
        compiler: builder.run(compiler),
        output,
    }
}

fn build_sysroot(builder: &Builder, compiler: Compiler, sysroot: Sysroot) -> BuiltCompilerAndSysroot {
    let parent_libs = match sysroot.libs {
        Libs::Std => None,
        Libs::Rustc => Some(Libs::Std),
        Libs::CodegenLLVM => Some(Libs::Rustc),
    };
    let sysroot = BuiltSysroot {
        compiler,
        sysroot,
    };
    let path = sysroot.path();
    println!("Building sysroot in {:?}", sysroot.path());
    rm_rf(&path);
    if let Some(libs) = parent_libs {
        let parent = builder.run(CompilerAndSysroot {
            compiler,
            sysroot: Sysroot {
                target: sysroot.sysroot.target,
                libs,
            },
        });
        t!(fs::create_dir_all(&path));
        cp_r(&parent.sysroot.path(), &path);
    }
    let libs = builder.run(CompilerAndLibs {
        compiler,
        sysroot: sysroot.sysroot,
    });
    let sysroot_libs = path.join("lib").join("rustlib").join(&sysroot.sysroot.target.0).join("lib");
    t!(fs::create_dir_all(&sysroot_libs));
    for f in &libs.output {
        copy(&f.0, &sysroot_libs.join(f.0.file_name().unwrap()));
        //println!("lib {:?}", f);
    }
    BuiltCompilerAndSysroot {
        compiler: builder.run(compiler),
        sysroot,
    }
}

tasks! {
    builder_var: builder;
    group TASKS;

    #[eval_always]
    pub task GitVersion {
        path: PathBuf,
    } -> Option<String> {
        crate::channel::GitInfo::new(false, &path).sha().map(|o| o.to_owned())
    }

    #[no_early_cutoff]
    //#[eval_always]
    pub task BootstrapSysroot {
        host: PlatformTriple,
    } -> PathBuf {
        let compiler = builder.run(Compiler {
            stage: 0,
            host,
        });
        let sysroot = build_dir().join(&host.0).join("bootstrap-sysroot");
        rm_rf(&sysroot);

        let bootstrap_libs = bootstrap_folder().join("lib").join("rustlib").join(&host.0).join("lib");
        let sysroot_libs = sysroot.join("lib").join("rustlib").join(&host.0).join("lib");

        t!(fs::create_dir_all(&sysroot_libs));
        for f in t!(fs::read_dir(&bootstrap_libs)).map(|f| t!(f)) {
            let filename = f.file_name();
            let filename = Path::new(&filename);

            const LIBS: &[&str] = &[
                "alloc",
                "std",
                "core",
                "compiler_builtins",
                "rustc_std_workspace_core",
                "libc",
                "rustc_demangle",
                "unwind",
                "panic_unwind",
                "backtrace_sys",
            ];

            let should_copy = LIBS.iter().any(|lib| {
                filename.extension().map(|e| e.to_str().unwrap()) == Some("rlib") &&
                filename.file_name().unwrap().to_str().unwrap().starts_with(&format!("lib{}-", lib))
            });

            if !should_copy {
                continue;
            }
            copy(&bootstrap_libs.join(filename), &sysroot_libs.join(filename));
        }
        sysroot
    }

    // Just a compiler with no libs
    #[no_early_cutoff]
    pub task Compiler {
        stage: u32,
        host: PlatformTriple,
    } -> BuiltCompiler {
        if stage == 0 {
            assert_eq!(host, host_triple());
            BuiltCompiler {
                compiler: Compiler {
                    stage,
                    host,
                },
            }
        } else {
            panic!()
        }
    }

    // A compiler and basic libraries: libstd, libtest, libproc_macro, etc.
    #[no_early_cutoff]
    //#[eval_always]
    task CompilerAndLibs {
        compiler: Compiler,
        sysroot: Sysroot,
    } -> BuiltLibs {
        assert!(compiler.stage != 0 || sysroot.libs != Libs::Std);
        build_libs(builder, compiler, sysroot)
    }

    // A compiler and basic libraries: libstd, libtest, libproc_macro, etc.
    #[no_early_cutoff]
    //#[eval_always]
    task CompilerAndSysroot {
        compiler: Compiler,
        sysroot: Sysroot,
    } -> BuiltCompilerAndSysroot {
        if compiler.stage == 0 && sysroot.libs == Libs::Std {
            assert_eq!(compiler.host, host_triple());
            assert_eq!(sysroot.target, host_triple());
            builder.run(BootstrapSysroot {
                    host: compiler.host,
            });
            BuiltCompilerAndSysroot {
                compiler: builder.run(compiler),
                sysroot: BuiltSysroot { sysroot, compiler },
            }
        } else {
            build_sysroot(builder, compiler, sysroot)
        }
    }
}

pub fn build(_config: Config) {
    let mut builder = Builder::load(&build_dir().join("build-index"));
    builder.handle_ctrlc();
    builder.register(TASKS);
    builder.register(cargo::TASKS);
    builder.register(llvm::TASKS);
    //builder.invalidate(llvm::LLVM { target: host_triple() });
    builder.run(llvm::LLVM { target: host_triple() });
    builder.run(CompilerAndSysroot {
        compiler: Compiler {
            stage: 0,
            host: host_triple(),
        },
        sysroot: Sysroot {
            libs: Libs::CodegenLLVM,
            target: host_triple(),
        },
    });
}
