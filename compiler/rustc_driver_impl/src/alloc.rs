#[cfg(target_os = "macos")]
mod interpose {
    use std::ffi::{c_int, c_void};

    #[repr(C)]
    struct Interpose {
        new: *const c_void,
        old: *const c_void,
    }

    extern "C" {
        fn malloc(size: usize) -> *mut c_void;
        fn calloc(items: usize, size: usize) -> *mut c_void;
        fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
        fn free(ptr: *mut c_void);
        fn posix_memalign(ptr: *mut *mut c_void, size: usize, align: usize) -> c_int;
        fn aligned_alloc(size: usize, align: usize) -> *mut c_void;
    }

    #[used]
    #[link_section = "__DATA,__interpose"]
    static INTERPOSE_MALLOC: Interpose = Interpose {
        new: tikv_jemalloc_sys::malloc as *const _ as *const c_void,
        old: malloc as *const _ as *const c_void,
    };

    #[used]
    #[link_section = "__DATA,__interpose"]
    static INTERPOSE_CALLOC: Interpose = Interpose {
        new: tikv_jemalloc_sys::calloc as *const _ as *const c_void,
        old: calloc as *const _ as *const c_void,
    };

    #[used]
    #[link_section = "__DATA,__interpose"]
    static INTERPOSE_REALLOC: Interpose = Interpose {
        new: tikv_jemalloc_sys::realloc as *const _ as *const c_void,
        old: realloc as *const _ as *const c_void,
    };

    #[used]
    #[link_section = "__DATA,__interpose"]
    static INTERPOSE_FREE: Interpose = Interpose {
        new: tikv_jemalloc_sys::free as *const _ as *const c_void,
        old: free as *const _ as *const c_void,
    };

    #[used]
    #[link_section = "__DATA,__interpose"]
    static INTERPOSE_POSIX_MEMALIGN: Interpose = Interpose {
        new: tikv_jemalloc_sys::posix_memalign as *const _ as *const c_void,
        old: posix_memalign as *const _ as *const c_void,
    };

    #[used]
    #[link_section = "__DATA,__interpose"]
    static INTERPOSE_ALIGNED_ALLOC: Interpose = Interpose {
        new: tikv_jemalloc_sys::aligned_alloc as *const _ as *const c_void,
        old: aligned_alloc as *const _ as *const c_void,
    };
}
