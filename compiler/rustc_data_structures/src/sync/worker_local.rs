use crate::sync::Lock;
use std::cell::Cell;
use std::lazy::OnceCell;
use std::ops::Deref;
use std::sync::Arc;

#[cfg(not(parallel_compiler))]
use crate::sync::OneThread;
#[cfg(parallel_compiler)]
use {crate::sync::CacheAligned, std::sync::atomic};

/// A pointer to the `RegistryData` which uniquely identifies a registry.
/// This identifier can be reused if the registry gets freed.
#[derive(Clone, Copy, Eq, PartialEq)]
struct RegistryId(usize);

impl RegistryId {
    // This is `inline(never)` because it accesses `THREAD_DATA` and thread locals have
    // inlining bugs.
    #[inline(never)]
    /// Verifies that the current thread is associated with the registry and returns its unique
    /// index within the registry. This panics if the current thread is not associated with this
    /// registry.
    ///
    /// Note that there's a race possible where the identifer in `THREAD_DATA` could be reused
    /// so this can succeed from a different registry.
    #[cfg(parallel_compiler)]
    fn verify(self) -> usize {
        let id = THREAD_DATA.registry_id.get();
        let index = THREAD_DATA.index.get();

        // This fence prevents the `index` memory access from being moved below into the
        // basic block below. That would cause LLVM to create a second TLS lookup since
        // it does one per basic block.
        atomic::compiler_fence(atomic::Ordering::SeqCst);

        if id == self { index } else { panic!("Unable to verify registry association") }
    }
}

struct RegistryData {
    thread_limit: usize,
    threads: Lock<usize>,
}

/// Represents a list of threads which can access worker locals.
#[derive(Clone)]
pub struct Registry(Arc<RegistryData>);

thread_local! {
    /// The registry associated with the thread.
    /// This allows the `WorkerLocal` type to clone the registry in its constructor.
    static REGISTRY: OnceCell<Registry> = OnceCell::new();
}

struct ThreadData {
    registry_id: Cell<RegistryId>,
    index: Cell<usize>,
}

/// A thread local which contains the identifer of `REGISTRY` but allows for faster access.
/// It also holds the index of the current thread.
#[thread_local]
static THREAD_DATA: ThreadData =
    ThreadData { registry_id: Cell::new(RegistryId(0)), index: Cell::new(0) };

impl Registry {
    /// Creates a registry which can hold up to `thread_limit` threads.
    pub fn new(thread_limit: usize) -> Self {
        Registry(Arc::new(RegistryData { thread_limit, threads: Lock::new(0) }))
    }

    /// Gets the registry associated with the current thread. Panics if there's no such registry.
    pub fn current() -> Self {
        REGISTRY.with(|registry| registry.get().cloned().expect("No assocated registry"))
    }

    // This is `inline(never)` because it accesses `THREAD_DATA` and thread locals have
    // inlining bugs.
    #[inline(never)]
    /// Registers the current thread with the registry so worker locals can be used on it.
    /// Panics if the thread limit is hit or if the thread already as an associated registry.
    pub fn register(&self) {
        let mut threads = self.0.threads.lock();
        if *threads < self.0.thread_limit {
            REGISTRY.with(|registry| {
                if registry.get().is_some() {
                    drop(threads);
                    panic!("Thread already has a registry");
                }
                registry.set(self.clone()).ok();
                THREAD_DATA.registry_id.set(self.id());
                THREAD_DATA.index.set(*threads);
                *threads += 1;
            });
        } else {
            drop(threads);
            panic!("Thread limit reached");
        }
    }

    /// Gets the identifer of this registry.
    fn id(&self) -> RegistryId {
        RegistryId(&*self.0 as *const RegistryData as usize)
    }
}

/// Holds worker-locals values for each possible thread in a registry. You can only access the
/// worker local value through the `Deref` impl on the registry associated with the thread it was
/// created on. It will panic otherwise.
pub struct WorkerLocal<T> {
    #[cfg(not(parallel_compiler))]
    local: OneThread<T>,
    #[cfg(parallel_compiler)]
    locals: Box<[CacheAligned<T>]>,
    #[cfg(parallel_compiler)]
    registry: Registry,
}

// This is safe because the `deref` call will return a reference to a `T` unique to each thread
// or it will panic for threads without an associated local. So there isn't a need for `T` to do
// it's own synchronization. The `verify` method on `RegistryId` has an issue where the the id
// can be reused, but `WorkerLocal` has a reference to `Registry` which will prevent any reuse.
unsafe impl<T> Sync for WorkerLocal<T> {}

impl<T> WorkerLocal<T> {
    /// Creates a new worker local where the `initial` closure computes the
    /// value this worker local should take for each thread in the registry.
    #[inline]
    pub fn new<F: FnMut(usize) -> T>(mut initial: F) -> WorkerLocal<T> {
        #[cfg(parallel_compiler)]
        {
            let registry = Registry::current();
            WorkerLocal {
                locals: {
                    let locals: Vec<_> =
                        (0..registry.0.thread_limit).map(|i| CacheAligned(initial(i))).collect();
                    locals.into_boxed_slice()
                },
                registry,
            }
        }
        #[cfg(not(parallel_compiler))]
        {
            WorkerLocal { local: OneThread::new(initial(0)) }
        }
    }

    /// Returns the worker-local value for each thread
    #[inline]
    pub fn into_inner(self) -> Vec<T> {
        #[cfg(parallel_compiler)]
        {
            self.locals.into_vec().into_iter().map(|local| local.0).collect()
        }
        #[cfg(not(parallel_compiler))]
        {
            vec![OneThread::into_inner(self.local)]
        }
    }
}

impl<T> WorkerLocal<Vec<T>> {
    /// Joins the elements of all the worker locals into one Vec
    pub fn join(self) -> Vec<T> {
        self.into_inner().into_iter().flat_map(|v| v).collect()
    }
}

impl<T> Deref for WorkerLocal<T> {
    type Target = T;

    #[inline(always)]
    #[cfg(not(parallel_compiler))]
    fn deref(&self) -> &T {
        &*self.local
    }

    #[inline(always)]
    #[cfg(parallel_compiler)]
    fn deref(&self) -> &T {
        // This is safe because `verify` will only return values less than
        // `self.registry.thread_limit` which is the size of the `self.locals` array.
        unsafe { &self.locals.get_unchecked(self.registry.id().verify()).0 }
    }
}
