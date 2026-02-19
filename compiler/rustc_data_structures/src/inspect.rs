//! Lightweight inspection types for debugging and diagnostics.
//!
//! This module provides a `Value` enum that can represent Rust-like
//! data (enums/structs/tuple variants) as well as simple scalar values.
//! It is intentionally small and serde-free; it mirrors the previous
//! use of a MessagePack `Value` in this crate but adds structured
//! variants for Rust ADTs.

use std::borrow::Cow;
use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::fs::File;
use std::hash::Hash;
use std::marker::PhantomData;

use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};

use crate::fx::FxHashMap;
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

    Schema {
        id: SchemaId,
        values: Vec<Value>,
    },

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

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize)]
pub enum Schema {
    Struct { path: &'static str, fields: &'static [&'static str] },
    StructTuple { path: &'static str, field_count: u32 },
    Enum { path: &'static str, variant_name: &'static str, variant: EnumVariantSchema },
}

/// Describes a single enum variant instance.
#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Hash, Serialize)]
pub enum EnumVariantSchema {
    /// Unit variant (no fields).
    Unit,
    /// Named fields (struct-like variant).
    Named(&'static [&'static str]),
    /// Positional fields (tuple-like variant).
    Tuple(u32),
}

pub struct FileOffset(pub u64);

pub struct SchemaRef(UnsafeCell<Schema>);

impl SchemaRef {
    pub const fn new(schema: Schema) -> Self {
        SchemaRef(UnsafeCell::new(schema))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct SchemaId(pub u32);

pub struct SpanArgs {
    pub lo_or_index: u32,
    pub len_with_tag_or_marker: u16,
    pub ctxt_or_parent_or_marker: u16,
}

pub trait Write {
    fn write_u128(&mut self, value: u128);
}

pub struct StructureState<'a, CTX, W> {
    pub schema_list: FxHashMap<usize, (SchemaId, &'static SchemaRef)>,
    pub writer: W,
    pub span_value: &'a dyn Fn(SpanArgs, &mut StructureState<'_, CTX, W>) -> Value,
    pub def_path: &'a dyn Fn(u32, u32, &mut StructureState<'_, CTX, W>) -> Value,
    pub crate_num: &'a dyn Fn(u32, &mut StructureState<'_, CTX, W>) -> Value,
    pub _marker: PhantomData<&'a CTX>,
}

impl<'a, CTX, W: Write> StructureState<'a, CTX, W> {
    pub fn intern_schema(&mut self, schema: &'static SchemaRef) -> SchemaId {
        let key = schema as *const SchemaRef as usize;
        if let Some(id) = self.schema_list.get(&key) {
            return id.0;
        }

        let id = SchemaId(self.schema_list.len() as u32);
        self.schema_list.insert(key, (id, schema));
        id
    }
}
