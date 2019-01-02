//! The arena, a fast but limited type of allocator.
//!
//! Arenas are a type of allocator that destroy the objects within, all at
//! once, once the arena itself is destroyed. They do not support deallocation
//! of individual objects while the arena itself is still alive. The benefit
//! of an arena is very fast allocation; just a pointer bump.
//!
//! This crate implements `TypedArena`, a simple arena that can only hold
//! objects of a single type.

#![doc(html_logo_url = "https://www.rust-lang.org/logos/rust-logo-128x128-blk-v2.png",
       html_favicon_url = "https://doc.rust-lang.org/favicon.ico",
       html_root_url = "https://doc.rust-lang.org/nightly/",
       test(no_crate_inject, attr(deny(warnings))))]

#![feature(alloc)]
#![feature(core_intrinsics)]
#![feature(dropck_eyepatch)]
#![feature(nll)]
#![feature(raw_vec_internals)]
#![feature(stmt_expr_attributes)]
#![cfg_attr(test, feature(test))]

#![allow(deprecated)]

extern crate alloc;
extern crate rustc_data_structures;
extern crate rustc_rayon_core;

use rustc_data_structures::sync::{SharedWorkerLocal, WorkerLocal, Lock};
use rustc_data_structures::{likely, unlikely, cold_path};

use std::cell::{Cell, RefCell};
use std::cmp;
use std::marker::{PhantomData, Send};
use std::mem;
use std::ptr;
use std::slice;

use alloc::raw_vec::RawVec;

struct CurrentChunk<T> {
    /// A pointer to the next object to be allocated.
    ptr: Cell<*mut T>,

    /// A pointer to the end of the allocated area. When this pointer is
    /// reached, a new chunk is allocated.
    end: Cell<*mut T>,
}

impl<T> Default for CurrentChunk<T> {
    #[inline]
    fn default() -> Self {
        CurrentChunk {
            // We set both `ptr` and `end` to 0 so that the first call to
            // alloc() will trigger a grow().
            ptr: Cell::new(0 as *mut T),
            end: Cell::new(0 as *mut T),
        }
    }
}

impl<T> CurrentChunk<T> {
    unsafe fn alloc_unchecked(&self, units: usize) -> *mut T {
        let ptr = self.ptr.get();
        // Advance the pointer.
        self.ptr.set(ptr.offset(units as isize));
        ptr
    }

    // FIXME: Write a fn which check for enough space for fixed sizes given that bytes < PAGE
    fn available_bytes(&self) -> usize {
        self.end.get() as usize - self.ptr.get() as usize
    }

    /// Grows the arena.
    #[inline(always)]
    fn grow(&self, n: usize, chunks: &mut Vec<TypedArenaChunk<T>>) {
        unsafe {
            let (chunk, mut new_capacity);
            if let Some(last_chunk) = chunks.last_mut() {
                let used_bytes = self.ptr.get() as usize - last_chunk.start() as usize;
                let currently_used_cap = used_bytes / mem::size_of::<T>();
                if last_chunk.storage.reserve_in_place(currently_used_cap, n) {
                    self.end.set(last_chunk.end());
                    return;
                } else {
                    new_capacity = last_chunk.storage.cap();
                    loop {
                        new_capacity = new_capacity.checked_mul(2).unwrap();
                        if new_capacity >= currently_used_cap + n {
                            break;
                        }
                    }
                }
            } else {
                let elem_size = cmp::max(1, mem::size_of::<T>());
                new_capacity = cmp::max(n, PAGE / elem_size);
            }
            chunk = TypedArenaChunk::<T>::new(new_capacity);
            self.ptr.set(chunk.start());
            self.end.set(chunk.end());
            chunks.push(chunk);
        }
    }
}

/// An arena that can hold objects of only one type.
pub struct TypedArena<T> {
    /// Pointers to the current chunk
    current: CurrentChunk<T>,

    /// A vector of arena chunks.
    chunks: RefCell<Vec<TypedArenaChunk<T>>>,

    /// Marker indicating that dropping the arena causes its owned
    /// instances of `T` to be dropped.
    _own: PhantomData<T>,
}

struct TypedArenaChunk<T> {
    /// The raw storage for the arena chunk.
    storage: RawVec<T>,
}

impl<T> TypedArenaChunk<T> {
    #[inline]
    unsafe fn new(capacity: usize) -> TypedArenaChunk<T> {
        TypedArenaChunk {
            storage: RawVec::with_capacity(capacity),
        }
    }

    /// Destroys this arena chunk.
    #[inline]
    unsafe fn destroy(&mut self, len: usize) {
        // The branch on needs_drop() is an -O1 performance optimization.
        // Without the branch, dropping TypedArena<u8> takes linear time.
        if mem::needs_drop::<T>() {
            let mut start = self.start();
            // Destroy all allocated objects.
            for _ in 0..len {
                ptr::drop_in_place(start);
                start = start.offset(1);
            }
        }
    }

    // Returns a pointer to the first allocated object.
    #[inline]
    fn start(&self) -> *mut T {
        self.storage.ptr()
    }

    // Returns a pointer to the end of the allocated space.
    #[inline]
    fn end(&self) -> *mut T {
        unsafe {
            if mem::size_of::<T>() == 0 {
                // A pointer as large as possible for zero-sized elements.
                !0 as *mut T
            } else {
                self.start().add(self.storage.cap())
            }
        }
    }
}

const PAGE: usize = 4096;

impl<T> Default for TypedArena<T> {
    /// Creates a new `TypedArena`.
    fn default() -> TypedArena<T> {
        TypedArena {
            current: CurrentChunk::default(),
            chunks: RefCell::new(vec![]),
            _own: PhantomData,
        }
    }
}

impl<T> TypedArena<T> {
    /// Allocates an object in the `TypedArena`, returning a reference to it.
    #[inline]
    pub fn alloc(&self, object: T) -> &mut T {
        assert!(mem::size_of::<T>() != 0);

        // Pass `Self` as an argument to avoid putting the closure on the stack
        let alloc = |arena: &Self, object| {
            unsafe {
                let ptr = arena.current.alloc_unchecked(1);
                // Write into uninitialized memory.
                ptr::write(ptr, object);
                &mut *ptr
            }
        };

        unsafe {
            if likely!(self.current.ptr.get() != self.current.end.get()) {
                alloc(self, object)
            } else {
                // We move the object in this closure so if it has a destructor
                // the fast path need not have an unwind handler to destroy it
                cold_path(move || {
                    self.grow(1);
                    alloc(self, object)
                })
            }
        }
    }

    /// Allocates a slice of objects that are copied into the `TypedArena`, returning a mutable
    /// reference to it. Will panic if passed a zero-sized types.
    ///
    /// Panics:
    ///
    ///  - Zero-sized types
    ///  - Zero-length slices
    #[inline]
    pub fn alloc_slice(&self, slice: &[T]) -> &mut [T]
    where
        T: Copy,
    {
        assert!(mem::size_of::<T>() != 0);

        let at_least_bytes = slice.len() * mem::size_of::<T>();
        if unlikely!(at_least_bytes > self.current.available_bytes()) {
            cold_path(|| {
                self.grow(slice.len())
            });
        }

        unsafe {
            let start_ptr = self.current.alloc_unchecked(slice.len());
            slice.as_ptr().copy_to_nonoverlapping(start_ptr, slice.len());
            slice::from_raw_parts_mut(start_ptr, slice.len())
        }
    }

    /// Grows the arena.
    #[inline(always)]
    fn grow(&self, n: usize) {
        self.current.grow(n, &mut *self.chunks.borrow_mut());
    }

    /// Clears the arena. Deallocates all but the longest chunk which may be reused.
    pub fn clear(&mut self) {
        unsafe {
            // Clear the last chunk, which is partially filled.
            let mut chunks_borrow = self.chunks.borrow_mut();
            if let Some(mut last_chunk) = chunks_borrow.last_mut() {
                self.clear_last_chunk(&mut last_chunk);
                let len = chunks_borrow.len();
                // If `T` is ZST, code below has no effect.
                for mut chunk in chunks_borrow.drain(..len-1) {
                    let cap = chunk.storage.cap();
                    chunk.destroy(cap);
                }
            }
        }
    }

    // Drops the contents of the last chunk. The last chunk is partially empty, unlike all other
    // chunks.
    fn clear_last_chunk(&self, last_chunk: &mut TypedArenaChunk<T>) {
        // Determine how much was filled.
        let start = last_chunk.start() as usize;
        // We obtain the value of the pointer to the first uninitialized element.
        let end = self.current.ptr.get() as usize;
        // We then calculate the number of elements to be dropped in the last chunk,
        // which is the filled area's length.
        let diff = if mem::size_of::<T>() == 0 {
            // `T` is ZST. It can't have a drop flag, so the value here doesn't matter. We get
            // the number of zero-sized values in the last and only chunk, just out of caution.
            // Recall that `end` was incremented for each allocated value.
            end - start
        } else {
            (end - start) / mem::size_of::<T>()
        };
        // Pass that to the `destroy` method.
        unsafe {
            last_chunk.destroy(diff);
        }
        // Reset the chunk.
        self.current.ptr.set(last_chunk.start());
    }
}

unsafe impl<#[may_dangle] T> Drop for TypedArena<T> {
    fn drop(&mut self) {
        unsafe {
            // Determine how much was filled.
            let mut chunks_borrow = self.chunks.borrow_mut();
            if let Some(mut last_chunk) = chunks_borrow.pop() {
                // Drop the contents of the last chunk.
                self.clear_last_chunk(&mut last_chunk);
                // The last chunk will be dropped. Destroy all other chunks.
                for chunk in chunks_borrow.iter_mut() {
                    let cap = chunk.storage.cap();
                    chunk.destroy(cap);
                }
            }
            // RawVec handles deallocation of `last_chunk` and `self.chunks`.
        }
    }
}

unsafe impl<T: Send> Send for TypedArena<T> {}

const UNIT_SIZE: usize = std::mem::size_of::<usize>();

#[inline(always)]
fn bytes_in_units(bytes: usize) -> usize {
    // FIXME: This addition could overflow
    (bytes + UNIT_SIZE - 1) / UNIT_SIZE
}

#[derive(Default)]
pub struct DroplessArena {
    /// Pointers to the current chunk
    current: WorkerLocal<CurrentChunk<usize>>,

    /// A vector of arena chunks, one per worker
    chunks: Lock<SharedWorkerLocal<Vec<TypedArenaChunk<usize>>>>,
}

impl DroplessArena {
    pub fn in_arena<T: ?Sized>(&self, ptr: *const T) -> bool {
        let ptr = ptr as *const u8 as *mut usize;

        self.chunks.borrow().iter().any(|chunks| chunks.iter().any(|chunk| {
            chunk.start() <= ptr && ptr < chunk.end()
        }))
    }

    #[inline]
    fn alloc_raw_fixed<B, C>(&self, check: C, bytes: B, align: usize) -> &mut [u8]
    where
        B: Fn() -> usize,
        C: Fn(&Self) -> bool,
    {
        unsafe {
            assert!(align <= UNIT_SIZE);
            // FIXME: Check that `bytes` fit in a isize

            // FIXME: arith_offset could overflow here.
            // Find some way to guarantee this doesn't happen for small fixed size types
            // FIXME: Compare ptr and end by equality for sizes < usize?

            // Pass `Self` as an argument to avoid putting the closure on the stack
            let alloc = |arena: &Self, bytes| {
                let ptr = arena.current.alloc_unchecked(bytes_in_units(bytes));
                slice::from_raw_parts_mut(ptr as *mut u8, bytes)
            };

            if likely!(check(self)) {
                alloc(self, bytes())
            } else {
                cold_path(move || {
                    self.current.grow(bytes_in_units(bytes()), &mut *self.chunks.lock());
                    alloc(self, bytes())
                })
            }
        }
    }

    #[inline]
    pub fn alloc_raw(&self, bytes: usize, align: usize) -> &mut [u8] {
        self.alloc_raw_fixed(
            |this: &Self| bytes <= this.current.available_bytes(),
            || bytes,
            align
        )
    }

    #[inline]
    pub fn alloc<T>(&self, object: T) -> &mut T {
        assert!(!mem::needs_drop::<T>());

        let mem = self.alloc_raw_fixed(
            |this| {
                // FIXME: Compare ptr and end by equality for sizes < usize?
                mem::size_of::<T>() <= this.current.available_bytes()
            },
            || mem::size_of::<T>(),
            mem::align_of::<T>()
        ) as *mut _ as *mut T;

        unsafe {
            // Write into uninitialized memory.
            ptr::write(mem, object);
            &mut *mem
        }
    }

    /// Allocates a slice of objects that are copied into the `DroplessArena`, returning a mutable
    /// reference to it. Will panic if passed a zero-sized type.
    ///
    /// Panics:
    ///
    ///  - Zero-sized types
    ///  - Zero-length slices
    #[inline]
    pub fn alloc_slice<T>(&self, slice: &[T]) -> &mut [T]
    where
        T: Copy,
    {
        assert!(!mem::needs_drop::<T>());

        let mem = self.alloc_raw(
            slice.len() * mem::size_of::<T>(),
            mem::align_of::<T>()) as *mut _ as *mut T;

        unsafe {
            slice.as_ptr().copy_to_nonoverlapping(mem, slice.len());
            slice::from_raw_parts_mut(mem, slice.len())
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate test;
    use self::test::Bencher;
    use super::TypedArena;
    use std::cell::Cell;

    #[allow(dead_code)]
    #[derive(Debug, Eq, PartialEq)]
    struct Point {
        x: i32,
        y: i32,
        z: i32,
    }

    #[test]
    pub fn test_unused() {
        let arena: TypedArena<Point> = TypedArena::default();
        assert!(arena.chunks.borrow().is_empty());
    }

    #[test]
    fn test_arena_alloc_nested() {
        struct Inner {
            value: u8,
        }
        struct Outer<'a> {
            inner: &'a Inner,
        }
        enum EI<'e> {
            I(Inner),
            O(Outer<'e>),
        }

        struct Wrap<'a>(TypedArena<EI<'a>>);

        impl<'a> Wrap<'a> {
            fn alloc_inner<F: Fn() -> Inner>(&self, f: F) -> &Inner {
                let r: &EI = self.0.alloc(EI::I(f()));
                if let &EI::I(ref i) = r {
                    i
                } else {
                    panic!("mismatch");
                }
            }
            fn alloc_outer<F: Fn() -> Outer<'a>>(&self, f: F) -> &Outer {
                let r: &EI = self.0.alloc(EI::O(f()));
                if let &EI::O(ref o) = r {
                    o
                } else {
                    panic!("mismatch");
                }
            }
        }

        let arena = Wrap(TypedArena::default());

        let result = arena.alloc_outer(|| Outer {
            inner: arena.alloc_inner(|| Inner { value: 10 }),
        });

        assert_eq!(result.inner.value, 10);
    }

    #[test]
    pub fn test_copy() {
        let arena = TypedArena::default();
        for _ in 0..100000 {
            arena.alloc(Point { x: 1, y: 2, z: 3 });
        }
    }

    #[bench]
    pub fn bench_copy(b: &mut Bencher) {
        let arena = TypedArena::default();
        b.iter(|| arena.alloc(Point { x: 1, y: 2, z: 3 }))
    }

    #[bench]
    pub fn bench_copy_nonarena(b: &mut Bencher) {
        b.iter(|| {
            let _: Box<_> = Box::new(Point { x: 1, y: 2, z: 3 });
        })
    }

    #[allow(dead_code)]
    struct Noncopy {
        string: String,
        array: Vec<i32>,
    }

    #[test]
    pub fn test_noncopy() {
        let arena = TypedArena::default();
        for _ in 0..100000 {
            arena.alloc(Noncopy {
                string: "hello world".to_string(),
                array: vec![1, 2, 3, 4, 5],
            });
        }
    }

    #[test]
    pub fn test_typed_arena_zero_sized() {
        let arena = TypedArena::default();
        for _ in 0..100000 {
            arena.alloc(());
        }
    }

    #[test]
    pub fn test_typed_arena_clear() {
        let mut arena = TypedArena::default();
        for _ in 0..10 {
            arena.clear();
            for _ in 0..10000 {
                arena.alloc(Point { x: 1, y: 2, z: 3 });
            }
        }
    }

    #[bench]
    pub fn bench_typed_arena_clear(b: &mut Bencher) {
        let mut arena = TypedArena::default();
        b.iter(|| {
            arena.alloc(Point { x: 1, y: 2, z: 3 });
            arena.clear();
        })
    }

    // Drop tests

    struct DropCounter<'a> {
        count: &'a Cell<u32>,
    }

    impl<'a> Drop for DropCounter<'a> {
        fn drop(&mut self) {
            self.count.set(self.count.get() + 1);
        }
    }

    #[test]
    fn test_typed_arena_drop_count() {
        let counter = Cell::new(0);
        {
            let arena: TypedArena<DropCounter> = TypedArena::default();
            for _ in 0..100 {
                // Allocate something with drop glue to make sure it doesn't leak.
                arena.alloc(DropCounter { count: &counter });
            }
        };
        assert_eq!(counter.get(), 100);
    }

    #[test]
    fn test_typed_arena_drop_on_clear() {
        let counter = Cell::new(0);
        let mut arena: TypedArena<DropCounter> = TypedArena::default();
        for i in 0..10 {
            for _ in 0..100 {
                // Allocate something with drop glue to make sure it doesn't leak.
                arena.alloc(DropCounter { count: &counter });
            }
            arena.clear();
            assert_eq!(counter.get(), i * 100 + 100);
        }
    }

    thread_local! {
        static DROP_COUNTER: Cell<u32> = Cell::new(0)
    }

    struct SmallDroppable;

    impl Drop for SmallDroppable {
        fn drop(&mut self) {
            DROP_COUNTER.with(|c| c.set(c.get() + 1));
        }
    }

    #[test]
    fn test_typed_arena_drop_small_count() {
        DROP_COUNTER.with(|c| c.set(0));
        {
            let arena: TypedArena<SmallDroppable> = TypedArena::default();
            for _ in 0..100 {
                // Allocate something with drop glue to make sure it doesn't leak.
                arena.alloc(SmallDroppable);
            }
            // dropping
        };
        assert_eq!(DROP_COUNTER.with(|c| c.get()), 100);
    }

    #[bench]
    pub fn bench_noncopy(b: &mut Bencher) {
        let arena = TypedArena::default();
        b.iter(|| {
            arena.alloc(Noncopy {
                string: "hello world".to_string(),
                array: vec![1, 2, 3, 4, 5],
            })
        })
    }

    #[bench]
    pub fn bench_noncopy_nonarena(b: &mut Bencher) {
        b.iter(|| {
            let _: Box<_> = Box::new(Noncopy {
                string: "hello world".to_string(),
                array: vec![1, 2, 3, 4, 5],
            });
        })
    }
}
