use crate::fx::{FxHashMap, FxHasher};
use crate::sync::{self, Lock, LockGuard};
use std::borrow::Borrow;
use std::collections::hash_map::RawEntryMut;
use std::hash::{Hash, Hasher};
use std::intrinsics::likely;
use std::mem::{self, ManuallyDrop, MaybeUninit};

#[derive(Clone, Default)]
#[cfg_attr(parallel_compiler, repr(align(64)))]
pub struct CacheAligned<T>(T);

#[cfg(parallel_compiler)]
// 32 shards is sufficient to reduce contention on an 8-core Ryzen 7 1700,
// but this should be tested on higher core count CPUs. How the `Sharded` type gets used
// may also affect the ideal number of shards.
const SHARD_BITS: usize = 5;

#[cfg(not(parallel_compiler))]
const SHARD_BITS: usize = 0;

pub const SHARDS: usize = 1 << SHARD_BITS;

/// An array of cache-line aligned inner locked structures with convenience methods.
//#[derive(Clone)]
pub struct Sharded<T> {
    mt: bool, // FIXME: Store a length here instead?
    shards: [MaybeUninit<CacheAligned<Lock<T>>>; SHARDS],
}

impl<T: Default> Default for Sharded<T> {
    #[inline]
    fn default() -> Self {
        Self::new(T::default)
    }
}

impl<T> Sharded<T> {
    #[inline]
    pub fn new(mut value: impl FnMut() -> T) -> Self {
        let mut result = ManuallyDrop::new(Sharded {
            mt: sync::STATE.get().active,
            shards: unsafe { MaybeUninit::uninit().assume_init() },
        });

        // FIXME: Drop created values if `value` panics.

        for i in 0..result.count() {
            result.shards[i] = MaybeUninit::new(CacheAligned(Lock::new(value())));
        }

        ManuallyDrop::into_inner(result)
    }

    fn count(&self) -> usize {
        if likely(!self.mt) {
            1
        } else {
            SHARDS
        }
    }

    #[inline]
    unsafe fn get_shard_by_index_unchecked(&self, i: usize) -> &Lock<T> {
        debug_assert!(i < self.count());
        let i = if SHARDS == 1 { 0 } else { i };
        &self.shards.get_unchecked(i).assume_init_ref().0
    }

    /// The shard is selected by hashing `val` with `FxHasher`.
    #[inline]
    pub fn get_shard_by_value<K: Hash + ?Sized>(&self, val: &K) -> &Lock<T> {
        if SHARDS == 1 {
            unsafe { self.get_shard_by_index_unchecked(0) }
        } else {
            self.get_shard_by_hash(make_hash(val))
        }
    }

    #[inline]
    pub fn get_shard_by_hash(&self, hash: u64) -> &Lock<T> {
        unsafe {
            if likely(!self.mt) {
                self.get_shard_by_index_unchecked(0)
            } else {
                self.get_shard_by_index_unchecked(get_shard_index_by_hash(hash))
            }
        }
    }

    #[inline]
    pub fn get_shard_by_index(&self, i: usize) -> &Lock<T> {
        debug_assert!(i < self.count());
        let i = if self.mt { i % SHARDS } else { 0 };
        unsafe { self.get_shard_by_index_unchecked(i) }
    }

    pub fn lock_shards(&self) -> Vec<LockGuard<'_, T>> {
        (0..self.count()).map(|i| self.get_shard_by_index(i).lock()).collect()
    }

    pub fn try_lock_shards(&self) -> Option<Vec<LockGuard<'_, T>>> {
        (0..self.count()).map(|i| self.get_shard_by_index(i).try_lock()).collect()
    }
}
/*
impl<T: Clone> Clone for Sharded<T> {
    #[inline(always)]
    fn clone(&self) -> Self {
        let mut result = ManuallyDrop::new(Sharded {
            mt: self.mt,
            shards: unsafe { MaybeUninit::uninit().assume_init() },
        });

        for i in 0..result.count() {
            result.shards[i] = self.get_shard_by_index();
        }

        result.into_inner()
    }
}
 */

unsafe impl<#[may_dangle] T> Drop for Sharded<T> {
    fn drop(&mut self) {
        for i in 0..self.count() {
            unsafe { self.shards[i].assume_init_drop() }
        }
    }
}

pub type ShardedHashMap<K, V> = Sharded<FxHashMap<K, V>>;

impl<K: Eq, V> ShardedHashMap<K, V> {
    pub fn len(&self) -> usize {
        self.lock_shards().iter().map(|shard| shard.len()).sum()
    }
}

impl<K: Eq + Hash + Copy> ShardedHashMap<K, ()> {
    #[inline]
    pub fn intern_ref<Q: ?Sized>(&self, value: &Q, make: impl FnOnce() -> K) -> K
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
    {
        let hash = make_hash(value);
        let mut shard = self.get_shard_by_hash(hash).lock();
        let entry = shard.raw_entry_mut().from_key_hashed_nocheck(hash, value);

        match entry {
            RawEntryMut::Occupied(e) => *e.key(),
            RawEntryMut::Vacant(e) => {
                let v = make();
                e.insert_hashed_nocheck(hash, v, ());
                v
            }
        }
    }

    #[inline]
    pub fn intern<Q>(&self, value: Q, make: impl FnOnce(Q) -> K) -> K
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
    {
        let hash = make_hash(&value);
        let mut shard = self.get_shard_by_hash(hash).lock();
        let entry = shard.raw_entry_mut().from_key_hashed_nocheck(hash, &value);

        match entry {
            RawEntryMut::Occupied(e) => *e.key(),
            RawEntryMut::Vacant(e) => {
                let v = make(value);
                e.insert_hashed_nocheck(hash, v, ());
                v
            }
        }
    }
}

pub trait IntoPointer {
    /// Returns a pointer which outlives `self`.
    fn into_pointer(&self) -> *const ();
}

impl<K: Eq + Hash + Copy + IntoPointer> ShardedHashMap<K, ()> {
    pub fn contains_pointer_to<T: Hash + IntoPointer>(&self, value: &T) -> bool {
        let hash = make_hash(&value);
        let shard = self.get_shard_by_hash(hash).lock();
        let value = value.into_pointer();
        shard.raw_entry().from_hash(hash, |entry| entry.into_pointer() == value).is_some()
    }
}

#[inline]
pub fn make_hash<K: Hash + ?Sized>(val: &K) -> u64 {
    let mut state = FxHasher::default();
    val.hash(&mut state);
    state.finish()
}

/// Get a shard with a pre-computed hash value. If `get_shard_by_value` is
/// ever used in combination with `get_shard_by_hash` on a single `Sharded`
/// instance, then `hash` must be computed with `FxHasher`. Otherwise,
/// `hash` can be computed with any hasher, so long as that hasher is used
/// consistently for each `Sharded` instance.
#[inline]
pub fn get_shard_index_by_hash(hash: u64) -> usize {
    let hash_len = mem::size_of::<usize>();
    // Ignore the top 7 bits as hashbrown uses these and get the next SHARD_BITS highest bits.
    // hashbrown also uses the lowest bits, so we can't use those
    let bits = (hash >> (hash_len * 8 - 7 - SHARD_BITS)) as usize;
    bits % SHARDS
}
