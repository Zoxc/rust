
fn def_path_value(crate_num: u32, index: u32) -> inspect::Value {
    // Construct a `DefId` from the supplied `u32` crate/def indices
    // and return an `inspect::Value::String` containing the def path.
    let def_id = rustc_span::def_id::DefId {
        krate: rustc_span::def_id::CrateNum::from_u32(crate_num),
        index: rustc_span::def_id::DefIndex::from_u32(index),
    };
    inspect::Value::String(with_no_trimmed_paths!(tcx.def_path_str(def_id).into()))
};

fn span_value(span_args){
    let span = Span::from_ags(span_args);

    const TAG_VALID_SPAN: u8 = 0;
    const TAG_INVALID_SPAN: u8 = 1;
    const TAG_RELATIVE_SPAN: u8 = 2;

    let span = span.data_untracked();
    span.ctxt.hash_stable(self, hasher);
    span.parent.hash_stable(self, hasher);

    if span.is_dummy() {
        return Value::Enum {
            path: std::borrow::Cow::Borrowed("Span"),
            variants: vec![EnumVariant::Unit (std::borrow::Cow::Borrowed("Dummy"))],
        }
    }

    let parent = span.parent.map(|parent| tcx.def_span(parent).data_untracked());
    if let Some(parent) = parent
        && parent.contains(span)
    {
        // This span is enclosed in a definition: only hash the relative position. This catches
        // a subset of the cases from the `file.contains(parent.lo)`. But we can do this check
        // cheaply without the expensive `span_data_to_lines_and_cols` query.
        Hash::hash(&TAG_RELATIVE_SPAN, hasher);
        (span.lo - parent.lo).to_u32().hash_stable(tcx, hasher);
        (span.hi - parent.lo).to_u32().hash_stable(tcx, hasher);
        return;
    }

    // If this is not an empty or invalid span, we want to hash the last position that belongs
    // to it, as opposed to hashing the first position past it.
    let Some((file, line_lo, col_lo, line_hi, col_hi)) =
        tcx.source_map().span_data_to_lines_and_cols(&span)
    else {
        Hash::hash(&TAG_INVALID_SPAN, hasher);
        return;
    };

    if let Some(parent) = parent
        && file.contains(parent.lo)
    {
        // This span is relative to another span in the same file,
        // only hash the relative position.
        Hash::hash(&TAG_RELATIVE_SPAN, hasher);
        Hash::hash(&(span.lo.0.wrapping_sub(parent.lo.0)), hasher);
        Hash::hash(&(span.hi.0.wrapping_sub(parent.lo.0)), hasher);
        return;
    }

    Hash::hash(&TAG_VALID_SPAN, hasher);
    Hash::hash(&file.stable_id, hasher);

    // Hash both the length and the end location (line/column) of a span. If we hash only the
    // length, for example, then two otherwise equal spans with different end locations will
    // have the same hash. This can cause a problem during incremental compilation wherein a
    // previous result for a query that depends on the end location of a span will be
    // incorrectly reused when the end location of the span it depends on has changed (see
    // issue #74890). A similar analysis applies if some query depends specifically on the
    // length of the span, but we only hash the end location. So hash both.

    let col_lo_trunc = (col_lo.0 as u64) & 0xFF;
    let line_lo_trunc = ((line_lo as u64) & 0xFF_FF_FF) << 8;
    let col_hi_trunc = (col_hi.0 as u64) & 0xFF << 32;
    let line_hi_trunc = ((line_hi as u64) & 0xFF_FF_FF) << 40;
    let col_line = col_lo_trunc | line_lo_trunc | col_hi_trunc | line_hi_trunc;
    let len = (span.hi - span.lo).0;
    Hash::hash(&col_line, hasher);
    Hash::hash(&len, hasher);
}

/// Collects structural representations for all queries and returns
/// them as an inspect::Value map: query_name -> { key -> value }
pub fn collect_all_query_structures<'tcx>(tcx: TyCtxt<'tcx>)
    -> inspect::Value
{
    let mut state = StructureState::<'_, ()> {
        def_path: &|| def_path_value(tcx),
        span_value: &|| span_value(tcx),
    };

    let mut out: Vec<(inspect::Value, inspect::Value)> = Vec::new();

    for f in PER_QUERY_COLLECT_STRUCTURES_FNS.iter() {
        let (name, map) = f(tcx, state);
        out.push((inspect::Value::String(name.into()), map));
    }

    // Convert Vec into BTreeMap for the Map variant
    let mut entries_map: ::std::collections::BTreeMap<inspect::Value, inspect::Value> =
        ::std::collections::BTreeMap::new();
    for (k, v) in out.into_iter() {
        entries_map.insert(k, v);
    }

    inspect::Value::Map(entries_map)
}
