use rustc_data_structures::fx::FxIndexMap;
use rustc_data_structures::stable_hasher::{HashStable, StableHasher, StructureState};
use rustc_span::Symbol;
use rustc_span::def_id::DefIdMap;

use crate::def_id::DefId;

#[derive(Debug, Default)]
pub struct DiagnosticItems {
    pub id_to_name: DefIdMap<Symbol>,
    pub name_to_id: FxIndexMap<Symbol, DefId>,
}

impl<CTX: crate::HashStableContext> HashStable<CTX> for DiagnosticItems {
    #[inline]
    fn hash_stable(&self, ctx: &mut CTX, hasher: &mut StableHasher) {
        self.name_to_id.hash_stable(ctx, hasher);
    }

    fn structure<'a>(
        &self,
        state: &mut StructureState<'a, CTX>,
        writer: &mut impl rustc_data_structures::inspect::Write,
    ) {
        // Represent by the name -> id mapping which is the primary data used for hashing,
        // but preserve the wrapper type.
        static SCHEMA: rustc_data_structures::inspect::SchemaRef =
            rustc_data_structures::inspect::SchemaRef::new(
                rustc_data_structures::inspect::Schema::Struct {
                    path: "rustc_hir::diagnostic_items::DiagnosticItems",
                    fields: &["name_to_id"],
                },
            );

        state.write_schema_header(&SCHEMA, writer);
        self.name_to_id.structure(state, writer);
    }
}
