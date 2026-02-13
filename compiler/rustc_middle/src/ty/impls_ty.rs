//! This module contains `HashStable` implementations for various data types
//! from `rustc_middle::ty` in no particular order.

use std::cell::RefCell;
use std::ptr;

use rustc_data_structures::fingerprint::Fingerprint;
use rustc_data_structures::fx::FxHashMap;
// Avoid importing `inspect::Value` unqualified to prevent collisions with
// `ty::Value` / `valtree::Value` in other modules. Use fully-qualified paths
// where the compact inspect `Value` is needed.
use rustc_data_structures::stable_hasher::{
    HashStable, HashingControls, StableHasher, StructureState, ToStableHashKey, rmpv,
};
use rustc_query_system::ich::StableHashingContext;
use tracing::trace;

use crate::middle::region;
use crate::{mir, ty};

impl<'a, 'tcx, H, T> HashStable<StableHashingContext<'a>> for &'tcx ty::list::RawList<H, T>
where
    T: HashStable<StableHashingContext<'a>>,
{
    fn hash_stable(&self, hcx: &mut StableHashingContext<'a>, hasher: &mut StableHasher) {
        // Note: this cache makes an *enormous* performance difference on certain benchmarks. E.g.
        // without it, compiling `diesel-2.2.10` can be 74% slower, and compiling
        // `deeply-nested-multi` can be ~4,000x slower(!)
        thread_local! {
            static CACHE: RefCell<FxHashMap<(*const (), HashingControls), Fingerprint>> =
                RefCell::new(Default::default());
        }

        let hash = CACHE.with(|cache| {
            let key = (ptr::from_ref(*self).cast::<()>(), hcx.hashing_controls());
            if let Some(&hash) = cache.borrow().get(&key) {
                return hash;
            }

            let mut hasher = StableHasher::new();
            self[..].hash_stable(hcx, &mut hasher);

            let hash: Fingerprint = hasher.finish();
            cache.borrow_mut().insert(key, hash);
            hash
        });

        hash.hash_stable(hcx, hasher);
    }

    fn structure(
        &self,
        _state: &mut StructureState<StableHashingContext<'a>>,
    ) -> ::rustc_data_structures::inspect::Value {
        // Represent the list structurally as an array of its element structures.
        ::rustc_data_structures::inspect::Value::Array(
            self[..].iter().map(|e| e.structure(_state)).collect(),
        )
    }
}

impl<'a, 'tcx, H, T> ToStableHashKey<StableHashingContext<'a>> for &'tcx ty::list::RawList<H, T>
where
    T: HashStable<StableHashingContext<'a>>,
{
    type KeyType = Fingerprint;

    #[inline]
    fn to_stable_hash_key(&self, hcx: &StableHashingContext<'a>) -> Fingerprint {
        let mut hasher = StableHasher::new();
        let mut hcx: StableHashingContext<'a> = hcx.clone();
        self.hash_stable(&mut hcx, &mut hasher);
        hasher.finish()
    }
}

impl<'a, 'tcx> HashStable<StableHashingContext<'a>> for ty::GenericArg<'tcx> {
    fn hash_stable(&self, hcx: &mut StableHashingContext<'a>, hasher: &mut StableHasher) {
        self.kind().hash_stable(hcx, hasher);
    }

    fn structure(
        &self,
        state: &mut StructureState<StableHashingContext<'a>>,
    ) -> ::rustc_data_structures::inspect::Value {
        // Delegate to the kind's structural representation
        self.kind().structure(state)
    }
}

// AllocIds get resolved to whatever they point to (to be stable)
impl<'a> HashStable<StableHashingContext<'a>> for mir::interpret::AllocId {
    fn hash_stable(&self, hcx: &mut StableHashingContext<'a>, hasher: &mut StableHasher) {
        ty::tls::with_opt(|tcx| {
            trace!("hashing {:?}", *self);
            let tcx = tcx.expect("can't hash AllocIds during hir lowering");
            tcx.try_get_global_alloc(*self).hash_stable(hcx, hasher);
        });
    }

    fn structure(
        &self,
        _state: &mut StructureState<StableHashingContext<'a>>,
    ) -> ::rustc_data_structures::inspect::Value {
        // We cannot access tcx here; represent AllocId by its resolved allocation's structure when available.
        // Fall back to a tag indicating AllocId.
        ::rustc_data_structures::inspect::Value::String("AllocId".into())
    }
}

impl<'a> HashStable<StableHashingContext<'a>> for mir::interpret::CtfeProvenance {
    fn hash_stable(&self, hcx: &mut StableHashingContext<'a>, hasher: &mut StableHasher) {
        self.into_parts().hash_stable(hcx, hasher);
    }

    fn structure(
        &self,
        state: &mut StructureState<StableHashingContext<'a>>,
    ) -> ::rustc_data_structures::inspect::Value {
        // Represent by its decomposed parts
        let (alloc, a, b) = self.into_parts();
        ::rustc_data_structures::inspect::Value::Array(vec![
            alloc.structure(state),
            a.structure(state),
            b.structure(state),
        ])
    }
}

impl<'a> ToStableHashKey<StableHashingContext<'a>> for region::Scope {
    type KeyType = region::Scope;

    #[inline]
    fn to_stable_hash_key(&self, _: &StableHashingContext<'a>) -> region::Scope {
        *self
    }
}
