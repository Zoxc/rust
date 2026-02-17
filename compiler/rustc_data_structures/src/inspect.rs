//! Lightweight inspection types for debugging and diagnostics.
//!
//! This module provides a `Value` enum that can represent Rust-like
//! data (enums/structs/tuple variants) as well as simple scalar values.
//! It is intentionally small and serde-free; it mirrors the previous
//! use of a MessagePack `Value` in this crate but adds structured
//! variants for Rust ADTs.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::hash::Hash;
use std::marker::PhantomData;

use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};

/// A compact representation of values for inspection purposes.
///
/// Models scalars and Rust aggregate types: `Struct`, `StructTuple` and
/// `Enum` (with `EnumVariant`).
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum Value {
    /// Boolean.
    Bool(bool),
    /// Integer (signed).
    Int(i128),
    /// Unsigned integer.
    UInt(u128),
    /// Floating point.
    F64(OrderedFloat<f64>),
    /// Binary blob.
    Binary(Vec<u8>),
    /// String-like value.
    String(Cow<'static, str>),
    /// Array of values.
    Array(Vec<Value>),
    Tuple(Vec<Value>),
    /// Map of key -> value.
    Map(BTreeMap<Value, Value>),

    /// Named-field struct value.
    Struct {
        path: Cow<'static, str>,
        fields: Vec<(Cow<'static, str>, Value)>,
    },

    /// Tuple struct / tuple variant value.
    StructTuple {
        path: Cow<'static, str>,
        fields: Vec<Value>,
    },

    /// Enum value: `path` is the enum path and `variant` describes the
    /// active variant.
    Enum {
        path: Cow<'static, str>,
        variant: EnumVariant,
    },
}

/// Describes a single enum variant instance.
#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub enum EnumVariant {
    /// Unit variant (no fields).
    Unit(Cow<'static, str>),
    /// Named fields (struct-like variant).
    Named(Cow<'static, str>, Vec<(Cow<'static, str>, Value)>),
    /// Positional fields (tuple-like variant).
    Tuple(Cow<'static, str>, Vec<Value>),
}

impl Value {
    /// Create a string Value from a &'static str without allocation.
    pub fn from_static_str(s: &'static str) -> Self {
        Value::String(Cow::Borrowed(s))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum Schema {
    Struct { path: Cow<'static, str>, fields: Vec<(Cow<'static, str>, Schema)> },
    StructTuple { path: Cow<'static, str>, fields: Vec<Schema> },
    Enum { path: Cow<'static, str>, variants: Vec<(Cow<'static, str>, Schema)> },
}

pub struct FileOffset(pub u64);

pub struct SchemaId(UnsafeCell<u8>);

impl SchemaId {
    pub const fn new() {
        SchemaId(UnsafeCell::new(0))
    }
}

pub struct SpanArgs {
    pub lo_or_index: u32,
    pub len_with_tag_or_marker: u16,
    pub ctxt_or_parent_or_marker: u16,
}

pub struct StructureState<'a, CTX> {
    schema_list: FxHashMap<Schema, FileOffset>,
    pub span_value: &'a dyn Fn(SpanArgs, &mut StructureState<'_, CTX>) -> Value,
    pub def_path: &'a dyn Fn(u32, u32, &mut StructureState<'_, CTX>) -> Value,
    pub crate_num: &'a dyn Fn(u32, &mut StructureState<'_, CTX>) -> Value,
    pub _marker: PhantomData<&'a CTX>,
}
