// Copyright 2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![allow(non_camel_case_types)]

use core::prelude::*;
use libc;
use collections::str::SendStr;

pub unsafe fn init() {
    imp::init();
}

pub struct Handler {
    _data: *mut libc::c_void
}

impl Handler {
    pub unsafe fn new() -> Handler {
        imp::make_handler()
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        unsafe {
            imp::drop_handler(self);
        }
    }
}

// get_task_info is called from an exception / signal handler.
// It steals the current task from local_ptr and neither returns it nor
// properly destroys it leaving the runtime in a broken state.
// This is fine since we'll exit the process soon after this is called.
unsafe fn get_task_info() -> Option<(uint, SendStr)> {
    use core::mem;
    use core::prelude::*;
    use alloc::boxed::Box;
    use local::Local;
    use task::Task;
    use collections::str::Slice;

    let task: Option<Box<Task>> = Local::try_take();

    task.map(|mut task| {
        let name = task.name.take();
        let runtime = task.take_runtime();
        let guard = runtime.stack_guard();

        // We don't want to invoke more things at this point.
        // We also don't want the string data destroyed.
        mem::forget(runtime);
        mem::forget(task);

        (guard.unwrap_or(0), name.unwrap_or(Slice("<unknown>")))
    })
}

#[cfg(windows)]
#[allow(uppercase_variables)]
mod imp {
    use core::prelude::*;
    use core::ptr;
    use core::mem;
    use libc;
    use libc::types::os::arch::extra::{LPVOID, DWORD, LONG};
    use stack;
    use super::{Handler, get_task_info};

    // This is initialized in init() and only read from after
    static mut PAGE_SIZE: uint = 0;

    #[no_split_stack]
    extern "system" fn vectored_handler(ExceptionInfo: *mut EXCEPTION_POINTERS) -> LONG {
        use core::uint;

        unsafe {
            let rec = &(*(*ExceptionInfo).ExceptionRecord);
            let code = rec.ExceptionCode;

            rterrln!("\nvectored_handler! {:x} addr {:x} sec {:x}", code, rec.ExceptionAddress as uint, rec.ExceptionInformation[1] as uint);
            if code != EXCEPTION_STACK_OVERFLOW &&
               code != EXCEPTION_ACCESS_VIOLATION {
                return EXCEPTION_CONTINUE_SEARCH;
            }

            // We're calling into functions with stack checks
            stack::record_os_managed_stack_bounds(0, uint::MAX);

            let task = get_task_info();

            if task.is_none() {
                return EXCEPTION_CONTINUE_SEARCH;
            }

            let (guard, name) = task.unwrap();

            rterrln!("guard page for {} is {:x}", name.as_slice(), guard);

            if code == EXCEPTION_ACCESS_VIOLATION {
                if guard == 0 {
                    return EXCEPTION_CONTINUE_SEARCH;
                }
                let addr = rec.ExceptionInformation[1] as uint;
            rterrln!("\n cmp {} vs {} for {}", addr, guard, name.as_slice());

                if addr < guard - PAGE_SIZE || addr >= guard {
                    return EXCEPTION_CONTINUE_SEARCH;
                }
            }
            
            rterrln!("\ntask '{}' has overflowed its stack", name.as_slice());

            EXCEPTION_CONTINUE_SEARCH
        }
    }

    pub unsafe fn init() {
        let mut info = mem::zeroed();
        libc::GetSystemInfo(&mut info);
        PAGE_SIZE = info.dwPageSize as uint;

        if AddVectoredExceptionHandler(0, vectored_handler) == ptr::mut_null() {
            fail!("Failed to install exception handler");
        }
    }

    pub unsafe fn make_handler() -> Handler {
        super::Handler { _data: 0i as *mut libc::c_void }
    }

    pub unsafe fn drop_handler(_handler: &mut Handler) {
    }

    pub struct EXCEPTION_RECORD {
        pub ExceptionCode: DWORD,
        pub ExceptionFlags: DWORD,
        pub ExceptionRecord: *mut EXCEPTION_RECORD,
        pub ExceptionAddress: LPVOID,
        pub NumberParameters: DWORD,
        pub ExceptionInformation: [LPVOID, ..EXCEPTION_MAXIMUM_PARAMETERS]
    }

    pub struct EXCEPTION_POINTERS {
        pub ExceptionRecord: *mut EXCEPTION_RECORD,
        pub ContextRecord: LPVOID
    }

    pub type PVECTORED_EXCEPTION_HANDLER = extern "system" fn(ExceptionInfo: *mut EXCEPTION_POINTERS) -> LONG;

    pub type ULONG = libc::c_ulong;

    static EXCEPTION_CONTINUE_SEARCH: LONG = 0;
    static EXCEPTION_MAXIMUM_PARAMETERS: uint = 15;
    static EXCEPTION_ACCESS_VIOLATION: DWORD = 0xc0000005;
    static EXCEPTION_STACK_OVERFLOW: DWORD = 0xc00000fd;

    #[allow(non_snake_case_functions)]
    extern "system" {
        fn AddVectoredExceptionHandler(FirstHandler: ULONG,
                                       VectoredHandler: PVECTORED_EXCEPTION_HANDLER)
                                      -> LPVOID;
    }
}

#[cfg(target_os = "linux")]
#[cfg(target_os = "macos")]
mod imp {
    use core::prelude::*;
    use stack;

    use super::{Handler, get_task_info};
    use core::mem;
    use core::ptr;
    use core::uint;
    use core::intrinsics;
    use self::signal::{siginfo, sigaction, SIGBUS, SIG_DFL,
                       SA_SIGINFO, SA_ONSTACK, sigaltstack,
                       SIGSTKSZ};
    use libc;
    use libc::funcs::posix88::mman::{mmap, munmap};
    use libc::consts::os::posix88::{SIGSEGV,
                                    PROT_READ,
                                    PROT_WRITE,
                                    MAP_PRIVATE,
                                    MAP_ANON,
                                    MAP_FAILED};


    // This is initialized in init() and only read from after
    static mut PAGE_SIZE: uint = 0;

    #[no_split_stack]
    unsafe extern fn signal_handler(signum: libc::c_int,
                                     info: *mut siginfo,
                                     _data: *mut libc::c_void) {
        unsafe fn term(signum: libc::c_int) {
            use core::mem::transmute;

            signal(signum, transmute(SIG_DFL));
            raise(signum);
            intrinsics::abort();
        }

        // We're calling into functions with stack checks
        stack::record_os_managed_stack_bounds(0, uint::MAX);

        let task = get_task_info();

        if task.is_none() {
            term(signum);
        }

        let (guard, name) = task.unwrap();
        let addr = (*info).si_addr as uint;

        if guard == 0 {
            term(signum);
        }

        if addr < guard - PAGE_SIZE || addr >= guard {
            term(signum);
        }

        rterrln!("\ntask '{}' has overflowed its stack", name.as_slice());

        intrinsics::abort();
    }

    pub unsafe fn init() {
        let psize = libc::sysconf(libc::consts::os::sysconf::_SC_PAGESIZE);
        if psize == -1 {
            fail!("Failed to get page size");
        }

        PAGE_SIZE = psize as uint;

        let mut action: sigaction = mem::zeroed();
        action.sa_flags = SA_SIGINFO | SA_ONSTACK;
        action.sa_sigaction = signal_handler as sighandler_t;
        sigaction(SIGSEGV, &action, ptr::mut_null());
        sigaction(SIGBUS, &action, ptr::mut_null());

        mem::forget(make_handler());
    }

    pub unsafe fn make_handler() -> Handler {
        let alt_stack = mmap(ptr::mut_null(),
                             signal::SIGSTKSZ,
                             PROT_READ | PROT_WRITE,
                             MAP_PRIVATE | MAP_ANON,
                             -1,
                             0);
        if alt_stack == MAP_FAILED {
            return Handler { _data: ptr::mut_null() }
        }

        let mut stack: sigaltstack = mem::zeroed();

        stack.ss_sp = alt_stack;
        stack.ss_flags = 0;
        stack.ss_size = SIGSTKSZ;

        sigaltstack(&stack, ptr::mut_null());

        Handler { _data: alt_stack }
    }

    pub unsafe fn drop_handler(handler: &mut Handler) {
        if handler._data != ptr::mut_null() {
            munmap(handler._data, SIGSTKSZ);
        }
    }

    type sighandler_t = *mut libc::c_void;

    #[cfg(target_os = "linux", target_arch = "x86")] // may not match
    #[cfg(target_os = "linux", target_arch = "x86_64")]
    #[cfg(target_os = "linux", target_arch = "arm")] // may not match
    #[cfg(target_os = "android")] // may not match
    mod signal {
        use libc;
        use super::sighandler_t;

        pub static SA_ONSTACK: libc::c_int = 0x08000000;
        pub static SA_SIGINFO: libc::c_int = 0x00000004;
        pub static SIGBUS: libc::c_int = 7;

        pub static SIGSTKSZ: libc::size_t = 8192;

        pub static SIG_DFL: sighandler_t = 0i as sighandler_t;

        // This definition is not as accurate as it could be, {si_addr} is
        // actually a giant union. Currently we're only interested in that field,
        // however.
        #[repr(C)]
        pub struct siginfo {
            si_signo: libc::c_int,
            si_errno: libc::c_int,
            si_code: libc::c_int,
            pub si_addr: *mut libc::c_void
        }

        #[repr(C)]
        pub struct sigaction {
            pub sa_sigaction: sighandler_t,
            pub sa_mask: sigset_t,
            pub sa_flags: libc::c_int,
            sa_restorer: *mut libc::c_void,
        }

        #[cfg(target_word_size = "32")]
        #[repr(C)]
        pub struct sigset_t {
            __val: [libc::c_ulong, ..32],
        }
        #[cfg(target_word_size = "64")]
        #[repr(C)]
        pub struct sigset_t {
            __val: [libc::c_ulong, ..16],
        }

        #[repr(C)]
        pub struct sigaltstack {
            pub ss_sp: *mut libc::c_void,
            pub ss_flags: libc::c_int,
            pub ss_size: libc::size_t
        }

    }

    #[cfg(target_os = "macos")]
    mod signal {
        use libc;
        use super::sighandler_t;

        pub static SA_ONSTACK: libc::c_int = 0x0001;
        pub static SA_SIGINFO: libc::c_int = 0x0040;
        pub static SIGBUS: libc::c_int = 10;

        pub static SIGSTKSZ: libc::size_t = 131072;

        pub static SIG_DFL: sighandler_t = 0i as sighandler_t;

        pub type sigset_t = u32;

        // This structure has more fields, but we're not all that interested in
        // them.
        #[repr(C)]
        pub struct siginfo {
            pub si_signo: libc::c_int,
            pub si_errno: libc::c_int,
            pub si_code: libc::c_int,
            pub pid: libc::pid_t,
            pub uid: libc::uid_t,
            pub status: libc::c_int,
            pub si_addr: *mut libc::c_void
        }

        #[repr(C)]
        pub struct sigaltstack {
            pub ss_sp: *mut libc::c_void,
            pub ss_size: libc::size_t,
            pub ss_flags: libc::c_int
        }

        #[repr(C)]
        pub struct sigaction {
            pub sa_sigaction: sighandler_t,
            pub sa_mask: sigset_t,
            pub sa_flags: libc::c_int,
        }
    }

    extern {
        pub fn signal(signum: libc::c_int, handler: sighandler_t) -> sighandler_t;
        pub fn raise(signum: libc::c_int) -> libc::c_int;

        pub fn sigaction(signum: libc::c_int,
                         act: *const sigaction,
                         oldact: *mut sigaction) -> libc::c_int;

        pub fn sigaltstack(ss: *const sigaltstack,
                           oss: *mut sigaltstack) -> libc::c_int;
    }
}

#[cfg(not(target_os = "linux"),
      not(target_os = "macos"),
      not(windows))]
mod imp {
    pub unsafe fn init() {
    }

    pub unsafe fn make_handler() -> super::Handler {
        super::Handler { _data: 0i as *mut libc::c_void }
    }

    pub unsafe fn drop_handler(_handler: &mut super::Handler) {
    }
}
