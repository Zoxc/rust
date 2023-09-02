//! This module implements a lock which only uses synchronization if `might_be_dyn_thread_safe` is true.
//! It implements `DynSend` and `DynSync` instead of the typical `Send` and `Sync` traits.
//!
//! When `cfg(parallel_compiler)` is not set, the lock is instead a wrapper around `RefCell`.

#![allow(dead_code)]

#[cfg(parallel_compiler)]
pub use maybe_sync::*;
#[cfg(not(parallel_compiler))]
pub use no_sync::*;

mod maybe_sync {
    use crate::sync::mode;
    use crate::sync::Mode;
    #[cfg(parallel_compiler)]
    use crate::sync::{DynSend, DynSync};
    use parking_lot::lock_api::RawRwLock as _;
    use parking_lot::RawRwLock;
    use std::cell::Cell;
    use std::cell::UnsafeCell;
    use std::fmt;
    use std::intrinsics::{likely, unlikely};
    use std::marker::PhantomData;
    use std::mem;
    use std::mem::ManuallyDrop;
    use std::ops::{Deref, DerefMut};
    use std::ptr::NonNull;

    /// A guard holding mutable access to a `RwLock` which is in a locked state.
    #[must_use = "if unused the RwLock will immediately unlock"]
    pub struct WriteGuard<'a, T: ?Sized> {
        // Use a pointer as we only have mutable access until we're dropped.
        data: NonNull<T>,
        lock: &'a RwLockInner,
        mode: Mode,
        // Make T invariant
        marker: PhantomData<&'a mut T>,
    }

    #[cfg(parallel_compiler)]
    impl<T> !DynSend for WriteGuard<'_, T> {}
    impl<T> !Send for WriteGuard<'_, T> {}

    impl<'a, T> WriteGuard<'a, T> {
        #[inline]
        fn new(lock: &'a RwLock<T>, mode: Mode) -> Self {
            WriteGuard {
                data: unsafe { NonNull::new_unchecked(lock.data.get()) },
                lock: &lock.inner,
                marker: PhantomData,
                mode,
            }
        }
    }

    impl<'a, T: ?Sized> WriteGuard<'a, T> {
        #[inline]
        pub fn map<U: ?Sized>(
            mut this: Self,
            f: impl FnOnce(&mut T) -> &mut U,
        ) -> WriteGuard<'a, U> {
            let guard = WriteGuard {
                data: NonNull::from(f(&mut *this)),
                lock: this.lock,
                mode: this.mode,
                marker: PhantomData,
            };
            // Don't run the destructor of the old guard
            mem::forget(this);
            guard
        }
    }

    impl<'a, T: ?Sized + 'a> Deref for WriteGuard<'a, T> {
        type Target = T;
        #[inline]
        fn deref(&self) -> &T {
            // SAFETY: We have shared access to the mutable access owned by this type,
            // so we can give out a shared reference.
            unsafe { &*self.data.as_ptr() }
        }
    }

    impl<'a, T: ?Sized + 'a> DerefMut for WriteGuard<'a, T> {
        #[inline]
        fn deref_mut(&mut self) -> &mut T {
            // SAFETY: We have mutable access to the data so we can give out a mutable reference.
            unsafe { &mut *self.data.as_ptr() }
        }
    }

    impl<'a, T: ?Sized + 'a> Drop for WriteGuard<'a, T> {
        #[inline]
        fn drop(&mut self) {
            // SAFETY (union access): We get `self.mode` from the lock operation so it is consistent
            // with the `lock.mode` state. This means we access the right union fields.
            unsafe {
                match self.mode {
                    Mode::NoSync => {
                        let cell = &self.lock.mode_union.no_sync;
                        debug_assert_eq!(cell.get(), WRITING);
                        cell.set(FREE);
                    }
                    // SAFETY (unlock_exclusive): We know that the lock is in a locked
                    // state because it is a invariant of this type.
                    Mode::Sync => self.lock.mode_union.sync.unlock_exclusive(),
                }
            }
        }
    }

    /// A guard holding shared access to a `RwLock`.
    #[must_use = "if unused the RwLock will immediately unlock"]
    pub struct ReadGuard<'a, T: ?Sized + 'a> {
        // Use a pointer as we only have shared access until we're dropped.
        data: NonNull<T>,
        lock: &'a RwLockInner,
        mode: Mode,
    }

    #[cfg(parallel_compiler)]
    impl<T> !DynSend for ReadGuard<'_, T> {}
    impl<T> !Send for ReadGuard<'_, T> {}

    impl<'a, T> ReadGuard<'a, T> {
        #[inline]
        fn new(lock: &'a RwLock<T>, mode: Mode) -> Self {
            ReadGuard {
                data: unsafe { NonNull::new_unchecked(lock.data.get()) },
                lock: &lock.inner,
                mode,
            }
        }
    }

    impl<'a, T: ?Sized> ReadGuard<'a, T> {
        #[inline]
        pub fn map<U: ?Sized>(this: Self, f: impl FnOnce(&T) -> &U) -> ReadGuard<'a, U> {
            let guard =
                ReadGuard { data: NonNull::from(f(&*this)), lock: this.lock, mode: this.mode };
            // Don't run the destructor of the old guard
            mem::forget(this);
            guard
        }
    }

    impl<'a, T: ?Sized + 'a> Deref for ReadGuard<'a, T> {
        type Target = T;
        #[inline]
        fn deref(&self) -> &T {
            // SAFETY: We have shared access to the shared access owned by this type,
            // so we can give out a shared reference.
            unsafe { &*self.data.as_ptr() }
        }
    }

    impl<'a, T: ?Sized + 'a> Drop for ReadGuard<'a, T> {
        #[inline]
        fn drop(&mut self) {
            // SAFETY (union access): We get `self.mode` from the lock operation so it is consistent
            // with the `lock.mode` state. This means we access the right union fields.
            unsafe {
                match self.mode {
                    Mode::NoSync => {
                        let cell = &self.lock.mode_union.no_sync;
                        // There must be readers left.
                        debug_assert!(cell.get() > 0);

                        cell.set(cell.get() - 1);
                    }
                    // SAFETY (unlock_shared): We know that the lock is in a locked
                    // state because it is a invariant of this type.
                    Mode::Sync => self.lock.mode_union.sync.unlock_shared(),
                }
            }
        }
    }

    /// No readers or writers for the cell.
    const FREE: isize = 0;

    /// there is a writer for the cell.
    const WRITING: isize = -1;

    #[inline(always)]
    fn try_get_reader_from_cell(cell: &Cell<isize>) -> bool {
        let next = cell.get().wrapping_add(1);
        // `next` will be negative if the number of readers overflow. It will be `0` if
        // there is a writer. In both these cases we can't allow a reader.
        let allowed = next > 0;
        if allowed {
            cell.set(next);
        }
        allowed
    }

    #[inline(always)]
    fn try_get_writer_from_cell(cell: &Cell<isize>) -> bool {
        // Allow a writer with no readers.
        let allowed = cell.get() == FREE;
        if likely(allowed) {
            cell.set(WRITING);
        }
        allowed
    }

    union ModeUnion {
        /// The number of reader of the cell. If the value is `WRITING` a writer borrows the cell.
        /// Only used if `RwLock.mode` is `NoSync`.
        no_sync: ManuallyDrop<Cell<isize>>,

        /// A lock implementation that's only used if `RwLock.mode` is `Sync`.
        sync: ManuallyDrop<RawRwLock>,
    }

    /// A raw lock which only uses synchronization if `might_be_dyn_thread_safe` is true.
    /// It contains no associated data and is used in the implementation of `RwLock` which does have such data.
    ///
    /// A manual implementation of a tagged union is used with the `mode` field and `ModeUnion` instead
    /// of using enums as it results in better code generation.
    struct RwLockInner {
        /// Indicates if synchronization is used via `mode_union.sync` if it's `Sync`, or if a
        /// not thread safe cell is used via `mode_union.no_sync` if it's `NoSync`.
        /// This is set on initialization and never changed.
        mode: Mode,

        mode_union: ModeUnion,
    }

    /// A lock which only uses synchronization if `might_be_dyn_thread_safe` is true.
    /// It implements `DynSend` and `DynSync` instead of the typical `Send` and `Sync`.
    pub struct RwLock<T> {
        inner: RwLockInner,
        data: UnsafeCell<T>,
    }

    impl<T> RwLock<T> {
        #[inline(always)]
        pub fn new(inner: T) -> Self {
            let (mode, mode_union) = if unlikely(mode::might_be_dyn_thread_safe()) {
                // Create the lock with synchronization enabled using the `RawRwLock` type.
                (Mode::Sync, ModeUnion { sync: ManuallyDrop::new(RawRwLock::INIT) })
            } else {
                // Create the lock with synchronization disabled.
                (Mode::NoSync, ModeUnion { no_sync: ManuallyDrop::new(Cell::new(FREE)) })
            };
            RwLock { inner: RwLockInner { mode, mode_union }, data: UnsafeCell::new(inner) }
        }

        #[inline(always)]
        pub fn get_mut(&mut self) -> &mut T {
            self.data.get_mut()
        }

        #[inline(always)]
        pub fn try_read(&self) -> Option<ReadGuard<'_, T>> {
            let mode = self.inner.mode;
            // SAFETY: This is safe since the union fields are used in accordance with `self.inner.mode`.
            unsafe {
                match mode {
                    Mode::NoSync => try_get_reader_from_cell(&self.inner.mode_union.no_sync),
                    Mode::Sync => self.inner.mode_union.sync.try_lock_shared(),
                }
                .then(|| ReadGuard::new(self, self.inner.mode))
            }
        }

        /// This acquires a read lock assuming syncronization is in a specific mode.
        ///
        /// Safety
        /// This method must only be called with `Mode::Sync` if `might_be_dyn_thread_safe` was
        /// true on lock creation.
        #[inline(always)]
        #[track_caller]
        pub unsafe fn read_assume(&self, mode: Mode) -> ReadGuard<'_, T> {
            #[inline(never)]
            #[track_caller]
            #[cold]
            fn read_failed() {
                panic!("unable to get read lock")
            }

            // SAFETY: This is safe since the union fields are used in accordance with `self.inner.mode`.
            unsafe {
                match mode {
                    Mode::NoSync => {
                        if unlikely(!try_get_reader_from_cell(&self.inner.mode_union.no_sync)) {
                            read_failed()
                        }
                    }
                    Mode::Sync => self.inner.mode_union.sync.lock_shared(),
                }
            }
            ReadGuard::new(self, mode)
        }

        #[inline(always)]
        #[track_caller]
        pub fn read(&self) -> ReadGuard<'_, T> {
            unsafe { self.read_assume(self.inner.mode) }
        }

        #[inline(always)]
        pub fn try_write(&self) -> Option<WriteGuard<'_, T>> {
            let mode = self.inner.mode;
            // SAFETY: This is safe since the union fields are used in accordance with `self.inner.mode`.
            unsafe {
                match mode {
                    Mode::NoSync => try_get_writer_from_cell(&self.inner.mode_union.no_sync),
                    Mode::Sync => self.inner.mode_union.sync.try_lock_exclusive(),
                }
            }
            .then(|| WriteGuard::new(self, self.inner.mode))
        }

        /// This acquires the write lock assuming syncronization is in a specific mode.
        ///
        /// Safety
        /// This method must only be called with `Mode::Sync` if `might_be_dyn_thread_safe` was
        /// true on lock creation.
        #[inline(always)]
        #[track_caller]
        pub unsafe fn write_assume(&self, mode: Mode) -> WriteGuard<'_, T> {
            #[inline(never)]
            #[track_caller]
            #[cold]
            fn write_failed() {
                panic!("unable to get write lock")
            }

            // SAFETY: This is safe since the union fields are used in accordance with `self.inner.mode`.
            unsafe {
                match mode {
                    Mode::NoSync => {
                        if unlikely(!try_get_writer_from_cell(&self.inner.mode_union.no_sync)) {
                            write_failed()
                        }
                    }
                    Mode::Sync => self.inner.mode_union.sync.lock_exclusive(),
                }
            }
            WriteGuard::new(self, mode)
        }

        #[inline(always)]
        #[track_caller]
        pub fn write(&self) -> WriteGuard<'_, T> {
            unsafe { self.write_assume(self.inner.mode) }
        }
    }

    #[cfg(parallel_compiler)]
    unsafe impl<T: DynSend> DynSend for RwLock<T> {}
    #[cfg(parallel_compiler)]
    unsafe impl<T: DynSend + DynSync> DynSync for RwLock<T> {}

    impl<T: fmt::Debug> fmt::Debug for RwLock<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.try_read() {
                Some(guard) => f.debug_struct("RwLock").field("data", &&*guard).finish(),
                None => {
                    struct LockedPlaceholder;
                    impl fmt::Debug for LockedPlaceholder {
                        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                            f.write_str("<locked>")
                        }
                    }

                    f.debug_struct("RwLock").field("data", &LockedPlaceholder).finish()
                }
            }
        }
    }

    impl<T: Default> Default for RwLock<T> {
        #[inline]
        fn default() -> Self {
            RwLock::new(T::default())
        }
    }
}

mod no_sync {
    use crate::sync::Mode;
    use std::cell::RefCell;

    pub use std::cell::Ref as ReadGuard;
    pub use std::cell::RefMut as WriteGuard;

    #[derive(Debug, Default)]
    pub struct RwLock<T>(RefCell<T>);

    impl<T> RwLock<T> {
        #[inline(always)]
        pub fn new(inner: T) -> Self {
            RwLock(RefCell::new(inner))
        }

        #[inline(always)]
        pub fn get_mut(&mut self) -> &mut T {
            self.0.get_mut()
        }

        #[inline(always)]
        #[track_caller]
        pub fn try_read(&self) -> Option<ReadGuard<'_, T>> {
            self.0.try_borrow().ok()
        }

        #[inline(always)]
        #[track_caller]
        // This is unsafe to match the API for the `parallel_compiler` case.
        pub unsafe fn read_assume(&self, _mode: Mode) -> ReadGuard<'_, T> {
            self.0.borrow()
        }

        #[inline(always)]
        #[track_caller]
        pub fn read(&self) -> ReadGuard<'_, T> {
            self.0.borrow()
        }

        #[inline(always)]
        #[track_caller]
        pub fn try_write(&self) -> Option<WriteGuard<'_, T>> {
            self.0.try_borrow_mut().ok()
        }

        #[inline(always)]
        #[track_caller]
        // This is unsafe to match the API for the `parallel_compiler` case.
        pub unsafe fn write_assume(&self, _mode: Mode) -> WriteGuard<'_, T> {
            self.0.borrow_mut()
        }

        #[inline(always)]
        #[track_caller]
        pub fn write(&self) -> WriteGuard<'_, T> {
            self.0.borrow_mut()
        }
    }
}

impl<T> RwLock<T> {
    #[inline(always)]
    #[track_caller]
    pub fn borrow(&self) -> ReadGuard<'_, T> {
        self.read()
    }

    #[inline(always)]
    #[track_caller]
    pub fn borrow_mut(&self) -> WriteGuard<'_, T> {
        self.write()
    }
}
