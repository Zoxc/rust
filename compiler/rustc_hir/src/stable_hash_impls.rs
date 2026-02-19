// Use fully-qualified `rustc_data_structures::inspect::Value` where needed to
// avoid accidental shadowing of other `Value` symbols.
use rustc_data_structures::inspect;
use rustc_data_structures::stable_hasher::{
    HashStable, StableHasher, StructureState, ToStableHashKey,
};
use rustc_span::def_id::DefPathHash;

use crate::HashIgnoredAttrId;
use crate::hir::{
    AttributeMap, BodyId, Crate, ForeignItemId, ImplItemId, ItemId, OwnerNodes, TraitItemId,
};
use crate::hir_id::ItemLocalId;
use crate::lints::DelayedLints;

/// Requirements for a `StableHashingContext` to be used in this crate.
/// This is a hack to allow using the `HashStable_Generic` derive macro
/// instead of implementing everything in `rustc_middle`.
pub trait HashStableContext: rustc_ast::HashStableContext + rustc_abi::HashStableContext {}

impl<HirCtx: crate::HashStableContext> ToStableHashKey<HirCtx> for BodyId {
    type KeyType = (DefPathHash, ItemLocalId);

    #[inline]
    fn to_stable_hash_key(&self, hcx: &HirCtx) -> (DefPathHash, ItemLocalId) {
        let BodyId { hir_id } = *self;
        hir_id.to_stable_hash_key(hcx)
    }
}

impl<HirCtx: crate::HashStableContext> ToStableHashKey<HirCtx> for ItemId {
    type KeyType = DefPathHash;

    #[inline]
    fn to_stable_hash_key(&self, hcx: &HirCtx) -> DefPathHash {
        self.owner_id.def_id.to_stable_hash_key(hcx)
    }
}

impl<HirCtx: crate::HashStableContext> ToStableHashKey<HirCtx> for TraitItemId {
    type KeyType = DefPathHash;

    #[inline]
    fn to_stable_hash_key(&self, hcx: &HirCtx) -> DefPathHash {
        self.owner_id.def_id.to_stable_hash_key(hcx)
    }
}

impl<HirCtx: crate::HashStableContext> ToStableHashKey<HirCtx> for ImplItemId {
    type KeyType = DefPathHash;

    #[inline]
    fn to_stable_hash_key(&self, hcx: &HirCtx) -> DefPathHash {
        self.owner_id.def_id.to_stable_hash_key(hcx)
    }
}

impl<HirCtx: crate::HashStableContext> ToStableHashKey<HirCtx> for ForeignItemId {
    type KeyType = DefPathHash;

    #[inline]
    fn to_stable_hash_key(&self, hcx: &HirCtx) -> DefPathHash {
        self.owner_id.def_id.to_stable_hash_key(hcx)
    }
}

// The following implementations of HashStable for `ItemId`, `TraitItemId`, and
// `ImplItemId` deserve special attention. Normally we do not hash `NodeId`s within
// the HIR, since they just signify a HIR nodes own path. But `ItemId` et al
// are used when another item in the HIR is *referenced* and we certainly
// want to pick up on a reference changing its target, so we hash the NodeIds
// in "DefPath Mode".

impl<'tcx, HirCtx: crate::HashStableContext> HashStable<HirCtx> for OwnerNodes<'tcx> {
    fn hash_stable(&self, hcx: &mut HirCtx, hasher: &mut StableHasher) {
        // We ignore the `nodes` and `bodies` fields since these refer to information included in
        // `hash` which is hashed in the collector and used for the crate hash.
        // `local_id_to_def_id` is also ignored because is dependent on the body, then just hashing
        // the body satisfies the condition of two nodes being different have different
        // `hash_stable` results.
        let OwnerNodes { opt_hash_including_bodies, nodes: _, bodies: _ } = *self;
        opt_hash_including_bodies.unwrap().hash_stable(hcx, hasher);
    }

    fn structure<W: rustc_data_structures::inspect::Write>(
        &self,
        state: &mut StructureState<'_, HirCtx, W>,
    ) -> inspect::Value {
        // Represent by the cached hash including bodies which is the canonical representation
        // used above for hashing, but preserve the wrapper type.
        let OwnerNodes { opt_hash_including_bodies, nodes: _, bodies: _ } = *self;
        inspect::Value::Struct {
            path: std::borrow::Cow::Borrowed(std::any::type_name::<Self>()),
            fields: vec![(
                std::borrow::Cow::Borrowed("hash"),
                opt_hash_including_bodies.structure(state),
            )],
        }
    }
}

impl<HirCtx: crate::HashStableContext> HashStable<HirCtx> for DelayedLints {
    fn hash_stable(&self, hcx: &mut HirCtx, hasher: &mut StableHasher) {
        let DelayedLints { opt_hash, .. } = *self;
        opt_hash.unwrap().hash_stable(hcx, hasher);
    }

    fn structure<W: rustc_data_structures::inspect::Write>(
        &self,
        state: &mut StructureState<'_, HirCtx, W>,
    ) -> inspect::Value {
        let DelayedLints { opt_hash, .. } = *self;
        inspect::Value::Struct {
            path: std::borrow::Cow::Borrowed(std::any::type_name::<Self>()),
            fields: vec![(std::borrow::Cow::Borrowed("hash"), opt_hash.structure(state))],
        }
    }
}

impl<'tcx, HirCtx: crate::HashStableContext> HashStable<HirCtx> for AttributeMap<'tcx> {
    fn hash_stable(&self, hcx: &mut HirCtx, hasher: &mut StableHasher) {
        // We ignore the `map` since it refers to information included in `opt_hash` which is
        // hashed in the collector and used for the crate hash.
        let AttributeMap { opt_hash, define_opaque: _, map: _ } = *self;
        opt_hash.unwrap().hash_stable(hcx, hasher);
    }

    fn structure<W: rustc_data_structures::inspect::Write>(
        &self,
        state: &mut StructureState<'_, HirCtx, W>,
    ) -> inspect::Value {
        let AttributeMap { opt_hash, define_opaque: _, map: _ } = *self;
        inspect::Value::Struct {
            path: std::borrow::Cow::Borrowed(std::any::type_name::<Self>()),
            fields: vec![(std::borrow::Cow::Borrowed("hash"), opt_hash.structure(state))],
        }
    }
}

impl<HirCtx: crate::HashStableContext> HashStable<HirCtx> for Crate<'_> {
    fn hash_stable(&self, hcx: &mut HirCtx, hasher: &mut StableHasher) {
        let Crate { owners: _, opt_hir_hash } = self;
        opt_hir_hash.unwrap().hash_stable(hcx, hasher)
    }

    fn structure<W: rustc_data_structures::inspect::Write>(
        &self,
        state: &mut StructureState<'_, HirCtx, W>,
    ) -> inspect::Value {
        let Crate { owners: _, opt_hir_hash } = self;
        inspect::Value::Struct {
            path: std::borrow::Cow::Borrowed(std::any::type_name::<Self>()),
            fields: vec![(std::borrow::Cow::Borrowed("hash"), opt_hir_hash.structure(state))],
        }
    }
}

impl<HirCtx: crate::HashStableContext> HashStable<HirCtx> for HashIgnoredAttrId {
    fn hash_stable(&self, _hcx: &mut HirCtx, _hasher: &mut StableHasher) {
        /* we don't hash HashIgnoredAttrId, we ignore them */
    }

    fn structure<W: rustc_data_structures::inspect::Write>(
        &self,
        _state: &mut StructureState<'_, HirCtx, W>,
    ) -> inspect::Value {
        // This value is ignored for hashing.
        inspect::Value::Enum {
            path: std::borrow::Cow::Borrowed(std::any::type_name::<Self>()),
            variant: inspect::EnumVariant::Unit(std::borrow::Cow::Borrowed("Ignored")),
        }
    }
}
