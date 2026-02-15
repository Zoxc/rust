use rustc_data_structures::inspect::{self, EnumVariant, Value};
use rustc_data_structures::stable_hasher::{SpanArgs, StructureState};
use rustc_middle::ty::TyCtxt;
use rustc_middle::ty::print::with_no_trimmed_paths;
use rustc_span::Span;
use std::borrow::Cow;
use std::hash::Hash;use crate::PER_QUERY_COLLECT_STRUCTURES_FNS;

fn def_path_value<'tcx>(tcx: TyCtxt<'tcx>, crate_num: u32, index: u32) -> inspect::Value {
    // Construct a `DefId` from the supplied `u32` crate/def indices
    // and return an `inspect::Value::String` containing the def path.
    let def_id = rustc_span::def_id::DefId {
        krate: rustc_span::def_id::CrateNum::from_u32(crate_num),
        index: rustc_span::def_id::DefIndex::from_u32(index),
    };
    inspect::Value::String(with_no_trimmed_paths!(tcx.def_path_str(def_id).into()))
}

fn span_value(tcx: TyCtxt<'_>, span_args:SpanArgs, state:&StructureState::<'_, ()>){
    let span = Span::from_ags(span_args);

    let span = span.data_untracked();
    let ctx_val =  span.ctxt.structure(state);
    let parent_val = span.parent.structure(state);

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
        let lo = (span.lo - parent.lo).to_u32();
        let hi = (span.hi - parent.lo).to_u32();
        return Value::Enum {
            path: Cow::Borrowed("Span"),
            variant: EnumVariant::Named(Cow::Borrowed("Relative",
                vec![
                    (Cow::Borrowed("ctxt"), ctx_val),
                    (Cow::Borrowed("parent"), parent_val),
                    (Cow::Borrowed("lo"), lo.structure(state)),
                    (Cow::Borrowed("hi"), hi.structure(state)),
                ]
            )),
        };
    }

    // If this is not an empty or invalid span, we want to hash the last position that belongs
    // to it, as opposed to hashing the first position past it.
    let Some((file, line_lo, col_lo, line_hi, col_hi)) =
        tcx.source_map().span_data_to_lines_and_cols(&span)
    else {
        return Value::Enum {
            path: std::borrow::Cow::Borrowed("Span"),
            variants: vec![EnumVariant::Unit (std::borrow::Cow::Borrowed("Dummy"))],
        }
    };

    if let Some(parent) = parent
        && file.contains(parent.lo)
    {
        // This span is relative to another span in the same file,
        // only hash the relative position.
        let lo = span.lo.0.wrapping_sub(parent.lo.0);
        let hi = span.hi.0.wrapping_sub(parent.lo.0);
        return Value::Enum {
            path: Cow::Borrowed("Span"),
            variant: EnumVariant::Named(Cow::Borrowed("Relative",
                vec![
                    (Cow::Borrowed("ctxt"), ctx_val),
                    (Cow::Borrowed("parent"), parent_val),
                    (Cow::Borrowed("lo"), lo.structure(state)),
                    (Cow::Borrowed("hi"), hi.structure(state)),
                ]
            )),
        };
        return;
    }

    // Hash both the length and the end location (line/column) of a span. If we hash only the
    // length, for example, then two otherwise equal spans with different end locations will
    // have the same hash. This can cause a problem during incremental compilation wherein a
    // previous result for a query that depends on the end location of a span will be
    // incorrectly reused when the end location of the span it depends on has changed (see
    // issue #74890). A similar analysis applies if some query depends specifically on the
    // length of the span, but we only hash the end location. So hash both.
    let len = (span.hi - span.lo).0;

    Value::Enum {
        path: Cow::Borrowed("Span"),
        variant: EnumVariant::Named(Cow::Borrowed("Valid",
            vec![
                (Cow::Borrowed("ctxt"), ctx_val),
                (Cow::Borrowed("parent"), parent_val),
                (Cow::Borrowed("stable_id"), file.stable_id.structure(state)),
                (Cow::Borrowed("col_lo"), col_lo.structure(state)),
                (Cow::Borrowed("col_hi"), col_hi.structure(state)),
                (Cow::Borrowed("line_lo"), line_lo.structure(state)),
                (Cow::Borrowed("line_hi"), line_hi.structure(state)),
                (Cow::Borrowed("len"), len.structure(state)),
            ]
        )),
    }
}

/// Collects structural representations for all queries and returns
/// them as an inspect::Value map: query_name -> { key -> value }
pub fn collect_all_query_structures<'tcx>(tcx: TyCtxt<'tcx>)
    -> inspect::Value
{
    let mut state = StructureState::<'_, ()> {
        def_path: &|crate_num, index| def_path_value(tcx,crate_num, index),
        span_value: &|args| span_value(tcx,args),
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
