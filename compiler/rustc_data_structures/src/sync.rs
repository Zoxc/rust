//! This module defines types which are thread safe if cfg!(parallel_compiler) is true.
//!
//! `Lrc` is an alias of `Arc` if cfg!(parallel_compiler) is true, `Rc` otherwise.
//!
//! `Lock` is a mutex.
//! It internally uses `parking_lot::Mutex` if cfg!(parallel_compiler) is true,
//! `RefCell` otherwise.
//!
//! `RwLock` is a read-write lock.
//! It internally uses `parking_lot::RwLock` if cfg!(parallel_compiler) is true,
//! `RefCell` otherwise.
//!
//! `MTLock` is a mutex which disappears if cfg!(parallel_compiler) is false.
//!
//! `MTRef` is an immutable reference if cfg!(parallel_compiler), and a mutable reference otherwise.
//!
//! `rustc_erase_owner!` erases a OwningRef owner into Erased or Erased + Send + Sync
//! depending on the value of cfg!(parallel_compiler).

use crate::owning_ref::{Erased, OwningRef};
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};
use std::ops::{Deref, DerefMut};

pub use std::sync::atomic::Ordering;
pub use std::sync::atomic::Ordering::SeqCst;

cfg_if! {
    if #[cfg(not(parallel_compiler))] {
        pub auto trait Send {}
        pub auto trait Sync {}

        impl<T: ?Sized> Send for T {}
        impl<T: ?Sized> Sync for T {}

        #[macro_export]
        macro_rules! rustc_erase_owner {
            ($v:expr) => {
                $v.erase_owner()
            }
        }

        use std::ops::Add;
        use std::panic::{resume_unwind, catch_unwind, AssertUnwindSafe};

        /// This is a single threaded variant of AtomicCell provided by crossbeam.
        /// Unlike `Atomic` this is intended for all `Copy` types,
        /// but it lacks the explicit ordering arguments.
        #[derive(Debug)]
        pub struct AtomicCell<T: Copy>(Cell<T>);

        impl<T: Copy> AtomicCell<T> {
            #[inline]
            pub fn new(v: T) -> Self {
                AtomicCell(Cell::new(v))
            }

            #[inline]
            pub fn get_mut(&mut self) -> &mut T {
                self.0.get_mut()
            }
        }

        impl<T: Copy> AtomicCell<T> {
            #[inline]
            pub fn into_inner(self) -> T {
                self.0.into_inner()
            }

            #[inline]
            pub fn load(&self) -> T {
                self.0.get()
            }

            #[inline]
            pub fn store(&self, val: T) {
                self.0.set(val)
            }

            #[inline]
            pub fn swap(&self, val: T) -> T {
                self.0.replace(val)
            }
        }

        /// This is a single threaded variant of `AtomicU64`, `AtomicUsize`, etc.
        /// It differs from `AtomicCell` in that it has explicit ordering arguments
        /// and is only intended for use with the native atomic types.
        /// You should use this type through the `AtomicU64`, `AtomicUsize`, etc, type aliases
        /// as it's not intended to be used separately.
        #[derive(Debug)]
        pub struct Atomic<T: Copy>(Cell<T>);

        impl<T: Copy> Atomic<T> {
            #[inline]
            pub fn new(v: T) -> Self {
                Atomic(Cell::new(v))
            }
        }

        impl<T: Copy> Atomic<T> {
            #[inline]
            pub fn into_inner(self) -> T {
                self.0.into_inner()
            }

            #[inline]
            pub fn load(&self, _: Ordering) -> T {
                self.0.get()
            }

            #[inline]
            pub fn store(&self, val: T, _: Ordering) {
                self.0.set(val)
            }

            #[inline]
            pub fn swap(&self, val: T, _: Ordering) -> T {
                self.0.replace(val)
            }
        }

        impl<T: Copy + PartialEq> Atomic<T> {
            #[inline]
            pub fn compare_exchange(&self,
                                    current: T,
                                    new: T,
                                    _: Ordering,
                                    _: Ordering)
                                    -> Result<T, T> {
                let read = self.0.get();
                if read == current {
                    self.0.set(new);
                    Ok(read)
                } else {
                    Err(read)
                }
            }
        }

        impl<T: Add<Output=T> + Copy> Atomic<T> {
            #[inline]
            pub fn fetch_add(&self, val: T, _: Ordering) -> T {
                let old = self.0.get();
                self.0.set(old + val);
                old
            }
        }

        pub type AtomicUsize = Atomic<usize>;
        pub type AtomicBool = Atomic<bool>;
        pub type AtomicU32 = Atomic<u32>;
        pub type AtomicU64 = Atomic<u64>;

        pub fn join<A, B, RA, RB>(oper_a: A, oper_b: B) -> (RA, RB)
            where A: FnOnce() -> RA,
                  B: FnOnce() -> RB
        {
            (oper_a(), oper_b())
        }

        pub struct SerialScope;

        impl SerialScope {
            pub fn spawn<F>(&self, f: F)
                where F: FnOnce(&SerialScope)
            {
                f(self)
            }
        }

        pub fn scope<F, R>(f: F) -> R
            where F: FnOnce(&SerialScope) -> R
        {
            f(&SerialScope)
        }

        #[macro_export]
        macro_rules! parallel {
            ($($blocks:tt),*) => {
                // We catch panics here ensuring that all the blocks execute.
                // This makes behavior consistent with the parallel compiler.
                let mut panic = None;
                $(
                    if let Err(p) = ::std::panic::catch_unwind(
                        ::std::panic::AssertUnwindSafe(|| $blocks)
                    ) {
                        if panic.is_none() {
                            panic = Some(p);
                        }
                    }
                )*
                if let Some(panic) = panic {
                    ::std::panic::resume_unwind(panic);
                }
            }
        }

        pub use std::iter::Iterator as ParallelIterator;

        pub fn par_iter<T: IntoIterator>(t: T) -> T::IntoIter {
            t.into_iter()
        }

        pub fn par_for_each_in<T: IntoIterator>(t: T, for_each: impl Fn(T::Item) + Sync + Send) {
            // We catch panics here ensuring that all the loop iterations execute.
            // This makes behavior consistent with the parallel compiler.
            let mut panic = None;
            t.into_iter().for_each(|i| {
                if let Err(p) = catch_unwind(AssertUnwindSafe(|| for_each(i))) {
                    if panic.is_none() {
                        panic = Some(p);
                    }
                }
            });
            if let Some(panic) = panic {
                resume_unwind(panic);
            }
        }

        pub type MetadataRef = OwningRef<Box<dyn Erased>, [u8]>;

        pub use std::rc::Rc as Lrc;
        pub use std::rc::Weak as Weak;
        pub use std::cell::Ref as ReadGuard;
        pub use std::cell::Ref as MappedReadGuard;
        pub use std::cell::RefMut as WriteGuard;
        pub use std::cell::RefMut as MappedWriteGuard;
        pub use std::cell::RefMut as MappedLockGuard;

        pub use std::lazy::OnceCell;

        use std::cell::RefCell as InnerRwLock;

        use std::cell::Cell;

        #[derive(Debug)]
        pub struct WorkerLocal<T>(OneThread<T>);

        impl<T> WorkerLocal<T> {
            /// Creates a new worker local where the `initial` closure computes the
            /// value this worker local should take for each thread in the thread pool.
            #[inline]
            pub fn new<F: FnMut(usize) -> T>(mut f: F) -> WorkerLocal<T> {
                WorkerLocal(OneThread::new(f(0)))
            }

            /// Returns the worker-local value for each thread
            #[inline]
            pub fn into_inner(self) -> Vec<T> {
                vec![OneThread::into_inner(self.0)]
            }
        }

        impl<T> Deref for WorkerLocal<T> {
            type Target = T;

            #[inline(always)]
            fn deref(&self) -> &T {
                &*self.0
            }
        }

        pub type MTRef<'a, T> = &'a mut T;

        #[derive(Debug, Default)]
        pub struct MTLock<T>(T);

        impl<T> MTLock<T> {
            #[inline(always)]
            pub fn new(inner: T) -> Self {
                MTLock(inner)
            }

            #[inline(always)]
            pub fn into_inner(self) -> T {
                self.0
            }

            #[inline(always)]
            pub fn get_mut(&mut self) -> &mut T {
                &mut self.0
            }

            #[inline(always)]
            pub fn lock(&self) -> &T {
                &self.0
            }

            #[inline(always)]
            pub fn lock_mut(&mut self) -> &mut T {
                &mut self.0
            }
        }

        // FIXME: Probably a bad idea (in the threaded case)
        impl<T: Clone> Clone for MTLock<T> {
            #[inline]
            fn clone(&self) -> Self {
                MTLock(self.0.clone())
            }
        }
    } else {
        pub use std::marker::Send as Send;
        pub use std::marker::Sync as Sync;

        pub use parking_lot::RwLockReadGuard as ReadGuard;
        pub use parking_lot::MappedRwLockReadGuard as MappedReadGuard;
        pub use parking_lot::RwLockWriteGuard as WriteGuard;
        pub use parking_lot::MappedRwLockWriteGuard as MappedWriteGuard;

        pub use parking_lot::MutexGuard as LockGuard;
        pub use parking_lot::MappedMutexGuard as MappedLockGuard;

        pub use std::lazy::SyncOnceCell as OnceCell;

        pub use std::sync::atomic::{AtomicBool, AtomicUsize, AtomicU32, AtomicU64};

        pub use crossbeam_utils::atomic::AtomicCell;

        pub use std::sync::Arc as Lrc;
        pub use std::sync::Weak as Weak;

        pub type MTRef<'a, T> = &'a T;

        #[derive(Debug, Default)]
        pub struct MTLock<T>(Lock<T>);

        impl<T> MTLock<T> {
            #[inline(always)]
            pub fn new(inner: T) -> Self {
                MTLock(Lock::new(inner))
            }

            #[inline(always)]
            pub fn into_inner(self) -> T {
                self.0.into_inner()
            }

            #[inline(always)]
            pub fn get_mut(&mut self) -> &mut T {
                self.0.get_mut()
            }

            #[inline(always)]
            pub fn lock(&self) -> LockGuard<'_, T> {
                self.0.lock()
            }

            #[inline(always)]
            pub fn lock_mut(&self) -> LockGuard<'_, T> {
                self.lock()
            }
        }

        use parking_lot::Mutex as InnerLock;
        use parking_lot::RwLock as InnerRwLock;

        use std::thread;
        pub use rayon::{join, scope};

        /// Runs a list of blocks in parallel. The first block is executed immediately on
        /// the current thread. Use that for the longest running block.
        #[macro_export]
        macro_rules! parallel {
            (impl $fblock:tt [$($c:tt,)*] [$block:tt $(, $rest:tt)*]) => {
                parallel!(impl $fblock [$block, $($c,)*] [$($rest),*])
            };
            (impl $fblock:tt [$($blocks:tt,)*] []) => {
                ::rustc_data_structures::sync::scope(|s| {
                    $(
                        s.spawn(|_| $blocks);
                    )*
                    $fblock;
                })
            };
            ($fblock:tt, $($blocks:tt),*) => {
                // Reverse the order of the later blocks since Rayon executes them in reverse order
                // when using a single thread. This ensures the execution order matches that
                // of a single threaded rustc
                parallel!(impl $fblock [] [$($blocks),*]);
            };
        }

        pub use rayon_core::WorkerLocal;

        pub use rayon::iter::ParallelIterator;
        use rayon::iter::IntoParallelIterator;

        pub fn par_iter<T: IntoParallelIterator>(t: T) -> T::Iter {
            t.into_par_iter()
        }

        pub fn par_for_each_in<T: IntoParallelIterator>(
            t: T,
            for_each: impl Fn(T::Item) + Sync + Send,
        ) {
            t.into_par_iter().for_each(for_each)
        }

        pub type MetadataRef = OwningRef<Box<dyn Erased + Send + Sync>, [u8]>;

        /// This makes locks panic if they are already held.
        /// It is only useful when you are running in a single thread
        const ERROR_CHECKING: bool = false;

        #[macro_export]
        macro_rules! rustc_erase_owner {
            ($v:expr) => {{
                let v = $v;
                ::rustc_data_structures::sync::assert_send_val(&v);
                v.erase_send_sync_owner()
            }}
        }
    }
}

pub fn assert_sync<T: ?Sized + Sync>() {}
pub fn assert_send<T: ?Sized + Send>() {}
pub fn assert_send_val<T: ?Sized + Send>(_t: &T) {}
pub fn assert_send_sync_val<T: ?Sized + Sync + Send>(_t: &T) {}

pub trait HashMapExt<K, V> {
    /// Same as HashMap::insert, but it may panic if there's already an
    /// entry for `key` with a value not equal to `value`
    fn insert_same(&mut self, key: K, value: V);
}

impl<K: Eq + Hash, V: Eq, S: BuildHasher> HashMapExt<K, V> for HashMap<K, V, S> {
    fn insert_same(&mut self, key: K, value: V) {
        self.entry(key).and_modify(|old| assert!(*old == value)).or_insert(value);
    }
}

use  std::cell::UnsafeCell;

use  std::marker::PhantomData;
use std::fmt;

use lock_api::RawMutex;

/// An RAII implementation of a "scoped lock" of a mutex. When this structure is
/// dropped (falls out of scope), the lock will be unlocked.
///
/// The data protected by the mutex can be accessed through this guard via its
/// `Deref` and `DerefMut` implementations.
#[must_use = "if unused the Mutex will immediately unlock"]
pub struct LockGuard<'a, T> {
    lock: &'a Lock<T>,
    marker: PhantomData<&'a mut T>,
}

unsafe impl<'a, T: std::marker::Sync + 'a> std::marker::Sync for LockGuard<'a, T> {}

impl<'a, T: 'a> LockGuard<'a, T> {
    /*/// Returns a reference to the original `Mutex` object.
    pub fn mutex(s: &Self) -> &'a Mutex<R, T> {
        s.mutex
    }

    /// Makes a new `MappedMutexGuard` for a component of the locked data.
    ///
    /// This operation cannot fail as the `LockGuard` passed
    /// in already locked the mutex.
    ///
    /// This is an associated function that needs to be
    /// used as `LockGuard::map(...)`. A method would interfere with methods of
    /// the same name on the contents of the locked data.
    #[inline]
    pub fn map<U: ?Sized, F>(s: Self, f: F) -> MappedMutexGuard<'a, R, U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        let raw = &s.mutex.raw;
        let data = f(unsafe { &mut *s.mutex.data.get() });
        mem::forget(s);
        MappedMutexGuard {
            raw,
            data,
            marker: PhantomData,
        }
    }

    /// Attempts to make a new `MappedMutexGuard` for a component of the
    /// locked data. The original guard is returned if the closure returns `None`.
    ///
    /// This operation cannot fail as the `LockGuard` passed
    /// in already locked the mutex.
    ///
    /// This is an associated function that needs to be
    /// used as `LockGuard::try_map(...)`. A method would interfere with methods of
    /// the same name on the contents of the locked data.
    #[inline]
    pub fn try_map<U: ?Sized, F>(s: Self, f: F) -> Result<MappedMutexGuard<'a, R, U>, Self>
    where
        F: FnOnce(&mut T) -> Option<&mut U>,
    {
        let raw = &s.mutex.raw;
        let data = match f(unsafe { &mut *s.mutex.data.get() }) {
            Some(data) => data,
            None => return Err(s),
        };
        mem::forget(s);
        Ok(MappedMutexGuard {
            raw,
            data,
            marker: PhantomData,
        })
    }

    /// Temporarily unlocks the mutex to execute the given function.
    ///
    /// This is safe because `&mut` guarantees that there exist no other
    /// references to the data protected by the mutex.
    #[inline]
    pub fn unlocked<F, U>(s: &mut Self, f: F) -> U
    where
        F: FnOnce() -> U,
    {
        // Safety: A LockGuard always holds the lock.
        unsafe {
            s.mutex.raw.unlock();
        }
        defer!(s.mutex.raw.lock());
        f()
    }*/
}

impl<'a, T: 'a> Deref for LockGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T: 'a> DerefMut for LockGuard<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T: 'a> Drop for LockGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        if self.lock.mt {
            unsafe { self.lock.mt_lock.unlock() };
        } else {
            self.lock.st_lock.set(false);
        }
    }
}
/*
impl<'a, T: fmt::Debug + ?Sized + 'a> fmt::Debug for LockGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: fmt::Display + ?Sized + 'a> fmt::Display for LockGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}*/

//#[derive(Debug)]
pub struct Lock<T> {
    mt: bool,
    thread: usize,
    st_lock: Cell<bool>,
    mt_lock: parking_lot::RawMutex,
    data: UnsafeCell<T>,
}

thread_local! {
    static ACTIVE: Cell<bool> = Cell::new(false);
}

#[thread_local]
static THREAD: Cell<usize> = Cell::new(0);

#[inline(never)]
fn current_thread() -> usize {
    let thread = &THREAD;
    let val = thread.get();
    if unlikely!(val == 0) {
        thread.set(3);
        thread.get()
    } else {
        val
    }
}

unsafe impl<T: std::marker::Send> std::marker::Send for Lock<T> {}
unsafe impl<T: std::marker::Send> std::marker::Sync for Lock<T> {}

impl<T: fmt::Debug> fmt::Debug for Lock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => f.debug_struct("Lock").field("data", &&*guard).finish(),
            None => {
                struct LockedPlaceholder;
                impl fmt::Debug for LockedPlaceholder {
                    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("<locked>")
                    }
                }

                f.debug_struct("Lock")
                    .field("data", &LockedPlaceholder)
                    .finish()
            }
        }
    }
}

impl<T> Lock<T> {
    #[inline(always)]
    pub fn new(inner: T) -> Self {
        ACTIVE.with(|active| {
            if active.get() {
                Lock {
                    mt: true,
                    st_lock: Cell::new(false),
                    thread: 0,
                    data: UnsafeCell::new(inner),
                    mt_lock: parking_lot::RawMutex::INIT,
                }
            } else {
                Lock {
                    mt: false,
                    st_lock: Cell::new(false),
                    thread: current_thread(),
                    data: UnsafeCell::new(inner),
                    mt_lock: parking_lot::RawMutex::INIT,
                }
            }

        })
    }

    #[inline(always)]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }

    #[inline(always)]
    pub fn get_mut(&mut self) -> &mut T {
        unsafe { &mut *self.data.get() }
    }

    #[inline(always)]
    pub fn try_lock(&self) -> Option<LockGuard<'_, T>> {
        if self.mt {
            if self.mt_lock.try_lock() {
                Some(LockGuard {
                    lock: self,
                    marker: PhantomData,
                })
            } else {
                None
            }
        } else {
            assert!(self.thread == current_thread());
            if self.st_lock.get() {
                None
            } else {
                self.st_lock.set(true);
                Some(LockGuard {
                    lock: self,
                    marker: PhantomData,
                })
            }
        }
    }

    #[inline(always)]
    pub fn lock(&self) -> LockGuard<'_, T> {
        if self.mt {
            self.mt_lock.lock();
            LockGuard {
                lock: self,
                marker: PhantomData,
            }
        } else {
            assert!(self.thread == current_thread());
            if self.st_lock.get() {
                panic!("lock was already held")
            } else {
                self.st_lock.set(true);
                LockGuard {
                    lock: self,
                    marker: PhantomData,
                }
            }
        }
    }

    #[inline(always)]
    pub fn with_lock<F: FnOnce(&mut T) -> R, R>(&self, f: F) -> R {
        f(&mut *self.lock())
    }

    #[inline(always)]
    pub fn borrow(&self) -> LockGuard<'_, T> {
        self.lock()
    }

    #[inline(always)]
    pub fn borrow_mut(&self) -> LockGuard<'_, T> {
        self.lock()
    }
}

impl<T: Default> Default for Lock<T> {
    #[inline]
    fn default() -> Self {
        Lock::new(T::default())
    }
}

// FIXME: Probably a bad idea
impl<T: Clone> Clone for Lock<T> {
    #[inline]
    fn clone(&self) -> Self {
        Lock::new(self.borrow().clone())
    }
}

#[derive(Debug, Default)]
pub struct RwLock<T>(InnerRwLock<T>);

impl<T> RwLock<T> {
    #[inline(always)]
    pub fn new(inner: T) -> Self {
        RwLock(InnerRwLock::new(inner))
    }

    #[inline(always)]
    pub fn into_inner(self) -> T {
        self.0.into_inner()
    }

    #[inline(always)]
    pub fn get_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }

    #[cfg(not(parallel_compiler))]
    #[inline(always)]
    pub fn read(&self) -> ReadGuard<'_, T> {
        self.0.borrow()
    }

    #[cfg(parallel_compiler)]
    #[inline(always)]
    pub fn read(&self) -> ReadGuard<'_, T> {
        if ERROR_CHECKING {
            self.0.try_read().expect("lock was already held")
        } else {
            self.0.read()
        }
    }

    #[inline(always)]
    pub fn with_read_lock<F: FnOnce(&T) -> R, R>(&self, f: F) -> R {
        f(&*self.read())
    }

    #[cfg(not(parallel_compiler))]
    #[inline(always)]
    pub fn try_write(&self) -> Result<WriteGuard<'_, T>, ()> {
        self.0.try_borrow_mut().map_err(|_| ())
    }

    #[cfg(parallel_compiler)]
    #[inline(always)]
    pub fn try_write(&self) -> Result<WriteGuard<'_, T>, ()> {
        self.0.try_write().ok_or(())
    }

    #[cfg(not(parallel_compiler))]
    #[inline(always)]
    pub fn write(&self) -> WriteGuard<'_, T> {
        self.0.borrow_mut()
    }

    #[cfg(parallel_compiler)]
    #[inline(always)]
    pub fn write(&self) -> WriteGuard<'_, T> {
        if ERROR_CHECKING {
            self.0.try_write().expect("lock was already held")
        } else {
            self.0.write()
        }
    }

    #[inline(always)]
    pub fn with_write_lock<F: FnOnce(&mut T) -> R, R>(&self, f: F) -> R {
        f(&mut *self.write())
    }

    #[inline(always)]
    pub fn borrow(&self) -> ReadGuard<'_, T> {
        self.read()
    }

    #[inline(always)]
    pub fn borrow_mut(&self) -> WriteGuard<'_, T> {
        self.write()
    }
}

// FIXME: Probably a bad idea
impl<T: Clone> Clone for RwLock<T> {
    #[inline]
    fn clone(&self) -> Self {
        RwLock::new(self.borrow().clone())
    }
}

/// A type which only allows its inner value to be used in one thread.
/// It will panic if it is used on multiple threads.
#[derive(Debug)]
pub struct OneThread<T> {
    #[cfg(parallel_compiler)]
    thread: thread::ThreadId,
    inner: T,
}

#[cfg(parallel_compiler)]
unsafe impl<T> std::marker::Sync for OneThread<T> {}
#[cfg(parallel_compiler)]
unsafe impl<T> std::marker::Send for OneThread<T> {}

impl<T> OneThread<T> {
    #[inline(always)]
    fn check(&self) {
        #[cfg(parallel_compiler)]
        assert_eq!(thread::current().id(), self.thread);
    }

    #[inline(always)]
    pub fn new(inner: T) -> Self {
        OneThread {
            #[cfg(parallel_compiler)]
            thread: thread::current().id(),
            inner,
        }
    }

    #[inline(always)]
    pub fn into_inner(value: Self) -> T {
        value.check();
        value.inner
    }
}

impl<T> Deref for OneThread<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.check();
        &self.inner
    }
}

impl<T> DerefMut for OneThread<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.check();
        &mut self.inner
    }
}
