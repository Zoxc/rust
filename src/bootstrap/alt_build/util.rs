// Copyright 2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Various utility functions used throughout rustbuild.
//!
//! Simple things like testing the various filesystem operations here and there,
//! not a lot of interesting happenings here unfortunately.

use std::fs;
use std::io::ErrorKind;
use std::io::{self, Write};
use std::path::Path;
use std::str;
use std::time::{Instant, SystemTime};

use filetime::{self, FileTime};

/// A helper macro to `unwrap` a result except also print out details like:
///
/// * The file/line of the panic
/// * The expression that failed
/// * The error itself
///
/// This is currently used judiciously throughout the build system rather than
/// using a `Result` with `try!`, but this may change one day...
#[macro_export]
macro_rules! t {
    ($e:expr) => {
        match $e {
            Ok(e) => e,
            Err(e) => panic!("{} failed with {}", stringify!($e), e),
        }
    };
}

/// Copies the `src` directory recursively to `dst`. Both are assumed to exist
/// when this function is called.
pub fn cp_r(src: &Path, dst: &Path) {
    for f in t!(fs::read_dir(src)) {
        let f = t!(f);
        let path = f.path();
        let name = path.file_name().unwrap();
        let dst = dst.join(name);
        if t!(f.file_type()).is_dir() {
            t!(fs::create_dir_all(&dst));
            cp_r(&path, &dst);
        } else {
            let _ = fs::remove_file(&dst);
            copy(&path, &dst);
        }
    }
}

/// Copies a file from `src` to `dst`
pub fn copy(src: &Path, dst: &Path) {    
    #[cfg(unix)]
    use std::os::unix::fs::symlink as symlink_file;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_file;

    let _ = fs::remove_file(&dst);
    let metadata = t!(src.symlink_metadata());
    if metadata.file_type().is_symlink() {
        let link = t!(fs::read_link(src));
        t!(symlink_file(link, dst));
    } else if let Ok(()) = fs::hard_link(src, dst) {
        // Attempt to "easy copy" by creating a hard link
        // (symlinks don't work on windows), but if that fails
        // just fall back to a slow `copy` operation.
    } else {
        if let Err(e) = fs::copy(src, dst) {
            panic!("failed to copy `{}` to `{}`: {}", src.display(),
                    dst.display(), e)
        }
        t!(fs::set_permissions(dst, metadata.permissions()));
        let atime = FileTime::from_last_access_time(&metadata);
        let mtime = FileTime::from_last_modification_time(&metadata);
        t!(filetime::set_file_times(dst, atime, mtime));
    }
}

fn do_op<F>(path: &Path, desc: &str, mut f: F)
where
    F: FnMut(&Path) -> io::Result<()>,
{
    match f(path) {
        Ok(()) => {}
        // On windows we can't remove a readonly file, and git will often clone files as readonly.
        // As a result, we have some special logic to remove readonly files on windows.
        // This is also the reason that we can't use things like fs::remove_dir_all().
        Err(ref e) if cfg!(windows) && e.kind() == ErrorKind::PermissionDenied => {
            let mut p = t!(path.symlink_metadata()).permissions();
            p.set_readonly(false);
            t!(fs::set_permissions(path, p));
            f(path).unwrap_or_else(|e| {
                panic!("failed to {} {}: {}", desc, path.display(), e);
            })
        }
        Err(e) => {
            panic!("failed to {} {}: {}", desc, path.display(), e);
        }
    }
}

pub struct TimeIt(Instant);

/// Returns an RAII structure that prints out how long it took to drop.
pub fn timeit() -> TimeIt {
    TimeIt(Instant::now())
}

impl Drop for TimeIt {
    fn drop(&mut self) {
        let time = self.0.elapsed();
        println!(
            "\tfinished in {}.{:03}",
            time.as_secs(),
            time.subsec_nanos() / 1_000_000
        );
    }
}

/// An RAII structure that indicates all output until this instance is dropped
/// is part of the same group.
///
/// On Travis CI, these output will be folded by default, together with the
/// elapsed time in this block. This reduces noise from unnecessary logs,
/// allowing developers to quickly identify the error.
///
/// Travis CI supports folding by printing `travis_fold:start:<name>` and
/// `travis_fold:end:<name>` around the block. Time elapsed is recognized
/// similarly with `travis_time:[start|end]:<name>`. These are undocumented, but
/// can easily be deduced from source code of the [Travis build commands].
///
/// [Travis build commands]:
/// https://github.com/travis-ci/travis-build/blob/f603c0089/lib/travis/build/templates/header.sh
pub struct OutputFolder {
    name: String,
    start_time: SystemTime, // we need SystemTime to get the UNIX timestamp.
}

impl OutputFolder {
    /// Creates a new output folder with the given group name.
    pub fn new(name: String) -> OutputFolder {
        // "\r" moves the cursor to the beginning of the line, and "\x1b[0K" is
        // the ANSI escape code to clear from the cursor to end of line.
        // Travis seems to have trouble when _not_ using "\r\x1b[0K", that will
        // randomly put lines to the top of the webpage.
        print!(
            "travis_fold:start:{0}\r\x1b[0Ktravis_time:start:{0}\r\x1b[0K",
            name
        );
        OutputFolder {
            name,
            start_time: SystemTime::now(),
        }
    }
}

impl Drop for OutputFolder {
    fn drop(&mut self) {
        use std::time::*;
        use std::u64;

        fn to_nanos(duration: Result<Duration, SystemTimeError>) -> u64 {
            match duration {
                Ok(d) => d.as_secs() * 1_000_000_000 + d.subsec_nanos() as u64,
                Err(_) => u64::MAX,
            }
        }

        let end_time = SystemTime::now();
        let duration = end_time.duration_since(self.start_time);
        let start = self.start_time.duration_since(UNIX_EPOCH);
        let finish = end_time.duration_since(UNIX_EPOCH);
        println!(
            "travis_fold:end:{0}\r\x1b[0K\n\
             travis_time:end:{0}:start={1},finish={2},duration={3}\r\x1b[0K",
            self.name,
            to_nanos(start),
            to_nanos(finish),
            to_nanos(duration)
        );
        io::stdout().flush().unwrap();
    }
}
