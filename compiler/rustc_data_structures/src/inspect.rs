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
use std::io::Write as IoWrite;
use std::marker::PhantomData;

use ordered_float::OrderedFloat;
use rustc_hashes::Hash128;
use rustc_stable_hash::StableSipHasher128;
use serde::{Deserialize, Serialize};

use crate::fx::FxHashMap;
/// A compact representation of values for inspection purposes.
///
/// Models scalars, collections, and schema-backed aggregate values.
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

// SchemaRef is intended to be referenced from 'static locations and only
// accessed via shared references; the contained Schema is never mutated
// after creation. Marking SchemaRef as Sync is safe because callers only
// obtain shared references to the inner Schema through `get()`.
unsafe impl Sync for SchemaRef {}

impl SchemaRef {
    pub const fn new(schema: Schema) -> Self {
        SchemaRef(UnsafeCell::new(schema))
    }

    #[inline]
    pub fn get(&self) -> &Schema {
        // SAFETY: callers only obtain shared references to the contained
        // Schema; the UnsafeCell is used to allow a 'static reference to
        // be created at runtime when interning schemas. Accessing the
        // inner Schema through a shared reference is safe here.
        unsafe { &*self.0.get() }
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
    fn write_i128(&mut self, value: i128);
    fn write_bytes(&mut self, bytes: &[u8]);
}

impl<'a> Write for &'a mut dyn Write {
    #[inline]
    fn write_u128(&mut self, value: u128) {
        (&mut **self).write_u128(value)
    }

    #[inline]
    fn write_i128(&mut self, value: i128) {
        (&mut **self).write_i128(value)
    }

    #[inline]
    fn write_bytes(&mut self, bytes: &[u8]) {
        (&mut **self).write_bytes(bytes)
    }
}

/// A `inspect::Write` implementation that hashes the written bytes.
///
/// This is useful when callers need a deterministic, compact key for a
/// structural stream without materializing the full byte buffer.
pub struct Hasher(StableSipHasher128);

impl Hasher {
    #[inline]
    pub fn new() -> Self {
        Hasher(StableSipHasher128::new())
    }

    #[inline]
    pub fn finish(self) -> Hash128 {
        self.0.finish::<Hash128>()
    }
}

impl Default for Hasher {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Write for Hasher {
    #[inline]
    fn write_u128(&mut self, value: u128) {
        std::hash::Hasher::write(&mut self.0, &value.to_le_bytes());
    }

    #[inline]
    fn write_i128(&mut self, value: i128) {
        std::hash::Hasher::write(&mut self.0, &value.to_le_bytes());
    }

    #[inline]
    fn write_bytes(&mut self, bytes: &[u8]) {
        std::hash::Hasher::write(&mut self.0, bytes);
    }
}

/// A simple writer wrapper used by inspection/serialization code which
/// optionally writes to an underlying `File`.
///
/// This exists to provide a concrete `W` type that implements the
/// `inspect::Write` trait so callers can pass a known writer into
/// `StructureState<'_, CTX, W>` when a file export path is desired.
pub struct FileWriter(pub Option<File>);

impl FileWriter {
    /// Create a new `FileWriter` owning the optional `File`.
    pub fn new(file: Option<File>) -> Self {
        FileWriter(file)
    }

    /// Take the inner file out of the writer.
    pub fn into_inner(self) -> Option<File> {
        self.0
    }
}

impl Write for FileWriter {
    fn write_u128(&mut self, value: u128) {
        if let Some(f) = &mut self.0 {
            // Best-effort write: ignore I/O errors here, callers handle
            // higher-level failures via diagnostics. Write as little-endian.
            let _ = f.write_all(&value.to_le_bytes());
        }
    }

    fn write_i128(&mut self, value: i128) {
        if let Some(f) = &mut self.0 {
            let _ = f.write_all(&value.to_le_bytes());
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        if let Some(f) = &mut self.0 {
            let _ = f.write_all(bytes);
        }
    }
}

// Implement `Write` for an optional mutable file reference so callers can
// pass `Option<&mut File>` as the `W` type parameter on `StructureState`.
impl<'a> Write for Option<&'a mut File> {
    fn write_u128(&mut self, value: u128) {
        if let Some(f) = self {
            let _ = f.write_all(&value.to_le_bytes());
        }
    }

    fn write_i128(&mut self, value: i128) {
        if let Some(f) = self {
            let _ = f.write_all(&value.to_le_bytes());
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        if let Some(f) = self {
            let _ = f.write_all(bytes);
        }
    }
}

pub struct State<'a> {
    pub schema_list: FxHashMap<usize, (SchemaId, &'static SchemaRef)>,
    pub span_value: &'a dyn for<'s> Fn(SpanArgs, &mut State<'s>, &mut dyn Write),
    pub def_path: &'a dyn for<'s> Fn(u32, u32, &mut State<'s>, &mut dyn Write),
    pub crate_num: &'a dyn for<'s> Fn(u32, &mut State<'s>, &mut dyn Write),
}

pub struct StructureState<'a, CTX> {
    pub state: &'a mut State<'a>,
    _marker: PhantomData<&'a CTX>,
}

impl<'a, CTX, W> StructureState<'a, CTX, W> {
    pub fn join(state: &'a mut State<'a>, writer: &'a mut W) -> Self {
        StructureState { state, writer, _marker: PhantomData }
    }
    pub fn split(&mut self) -> (&mut State<'a>, &mut W) {
        (self.state, &mut self.writer)
    }
}

impl<'a, CTX, W: Write> StructureState<'a, CTX, W> {
    pub fn intern_schema(&mut self, schema: &'static SchemaRef) -> SchemaId {
        let key = schema as *const SchemaRef as usize;
        if let Some(id) = self.state.schema_list.get(&key) {
            return id.0;
        }

        let id = SchemaId(self.state.schema_list.len() as u32);
        self.state.schema_list.insert(key, (id, schema));
        id
    }

    #[inline]
    fn write_tag(&mut self, tag: u8) {
        self.writer.write_bytes(&[tag]);
    }

    pub fn write_bool(&mut self, v: bool) {
        self.write_tag(ValueKind::Bool);
        let byte = if v { 1u8 } else { 0u8 };
        self.writer.write_bytes(&[byte]);
    }

    pub fn write_int(&mut self, v: i128) {
        self.write_tag(ValueKind::Int);
        self.writer.write_i128(v);
    }

    pub fn write_uint(&mut self, v: u128) {
        self.write_tag(ValueKind::UInt);
        self.writer.write_u128(v);
    }

    pub fn write_f64(&mut self, v: f64) {
        self.write_tag(ValueKind::F64);
        self.writer.write_bytes(&v.to_bits().to_le_bytes());
    }

    pub fn write_binary(&mut self, v: &[u8]) {
        self.write_tag(ValueKind::Binary);
        self.writer.write_u128(v.len() as u128);
        self.writer.write_bytes(v);
    }

    pub fn write_string(&mut self, s: &str) {
        self.write_tag(ValueKind::String);
        self.writer.write_u128(s.len() as u128);
        self.writer.write_bytes(s.as_bytes());
    }

    pub fn write_array_header(&mut self, len: usize) {
        self.write_tag(ValueKind::Array);
        self.writer.write_u128(len as u128);
    }

    pub fn write_tuple_header(&mut self, len: usize) {
        self.write_tag(ValueKind::Tuple);
        self.writer.write_u128(len as u128);
    }

    pub fn write_map_header(&mut self, len: usize) {
        self.write_tag(ValueKind::Map);
        self.writer.write_u128(len as u128);
    }

    pub fn write_schema_header(&mut self, schema: &'static SchemaRef) {
        self.write_tag(ValueKind::Schema);
    }
}
