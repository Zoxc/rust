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
use std::io;
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

#[repr(u8)]
enum ValueKind {
    Bool = 0,
    Int = 1,
    UInt = 2,
    F64 = 3,
    Binary = 4,
    String = 5,
    Array = 6,
    Tuple = 7,
    Map = 8,
    Schema = 9,
}

#[inline]
fn write_tag<W: Write + ?Sized>(writer: &mut W, tag: u8) {
    writer.write_raw_bytes(&[tag]);
}

pub trait Write {
    fn write_raw_u128(&mut self, value: u128);
    fn write_raw_i128(&mut self, value: i128);
    fn write_raw_bytes(&mut self, bytes: &[u8]);

    fn write_bool(&mut self, v: bool) {
        write_tag(self, ValueKind::Bool as u8);
        let byte = if v { 1u8 } else { 0u8 };
        self.write_raw_bytes(&[byte]);
    }

    fn write_int(&mut self, v: i128) {
        write_tag(self, ValueKind::Int as u8);
        self.write_raw_i128(v);
    }

    fn write_uint(&mut self, v: u128) {
        write_tag(self, ValueKind::UInt as u8);
        self.write_raw_u128(v);
    }

    fn write_f64(&mut self, v: f64) {
        write_tag(self, ValueKind::F64 as u8);
        self.write_raw_bytes(&v.to_bits().to_le_bytes());
    }

    fn write_binary(&mut self, v: &[u8]) {
        write_tag(self, ValueKind::Binary as u8);
        self.write_raw_u128(v.len() as u128);
        self.write_raw_bytes(v);
    }

    fn write_string(&mut self, s: &str) {
        write_tag(self, ValueKind::String as u8);
        self.write_raw_u128(s.len() as u128);
        self.write_raw_bytes(s.as_bytes());
    }

    fn write_array_header(&mut self, len: usize) {
        write_tag(self, ValueKind::Array as u8);
        self.write_raw_u128(len as u128);
    }

    fn write_tuple_header(&mut self, len: usize) {
        write_tag(self, ValueKind::Tuple as u8);
        self.write_raw_u128(len as u128);
    }

    fn write_map_header(&mut self, len: usize) {
        write_tag(self, ValueKind::Map as u8);
        self.write_raw_u128(len as u128);
    }
}

impl<'a> Write for &'a mut dyn Write {
    #[inline]
    fn write_raw_u128(&mut self, value: u128) {
        (&mut **self).write_raw_u128(value)
    }

    #[inline]
    fn write_raw_i128(&mut self, value: i128) {
        (&mut **self).write_raw_i128(value)
    }

    #[inline]
    fn write_raw_bytes(&mut self, bytes: &[u8]) {
        (&mut **self).write_raw_bytes(bytes)
    }
}

/// A `inspect::Write` implementation that hashes the written bytes.
///
/// This is useful when callers need a deterministic, compact key for a
/// structural stream without materializing the full byte buffer.
pub struct Hasher(StableHasher);

impl Hasher {
    #[inline]
    pub fn new() -> Self {
        Hasher(StableHasher::new())
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
    fn write_raw_u128(&mut self, value: u128) {
        std::hash::Hasher::write(&mut self.0, &value.to_le_bytes());
    }

    #[inline]
    fn write_raw_i128(&mut self, value: i128) {
        std::hash::Hasher::write(&mut self.0, &value.to_le_bytes());
    }

    #[inline]
    fn write_raw_bytes(&mut self, bytes: &[u8]) {
        std::hash::Hasher::write(&mut self.0, bytes);
    }
}

/// A simple writer wrapper used by inspection/serialization code which
/// optionally writes to an underlying `File`.
///
/// This exists to provide a concrete `W` type that implements the
/// `inspect::Write` trait so callers can pass a known writer into
/// `StructureState<'_, CTX, W>` when a file export path is desired.
pub struct IoWriter<T: io::Write> {
    pub inner: T,
    pub err: Option<std::io::Error>,
}

impl<T> IoWriter<T> {
    /// Create a new `IoWriter` owning the optional `File`.
    pub fn new(file: File) -> Self {
        IoWriter { inner: file, err: None }
    }
    }

    /// Take the inner writer out of the IoWriter and return any cached I/O
    /// error that occurred while writing. The inner writer is always
    /// returned so callers can continue to operate on it if desired.
    pub fn into_inner(self) -> (T, Option<std::io::Error>) {
        (self.inner, self.err)
    }
}

impl<T: io::Write> Write for IoWriter<T> {
    fn write_raw_u128(&mut self, value: u128) {
        // Best-effort write: cache the first I/O error and continue. This
        // allows callers to attempt to flush/inspect the inner writer and
        // observe the error via `into_inner` when finished.
        if self.err.is_none() {
            if let Err(e) = self.inner.write_all(&value.to_le_bytes()) {
                self.err = Some(e);
            }
        }
    }

    fn write_raw_i128(&mut self, value: i128) {
        if self.err.is_none() {
            if let Err(e) = self.inner.write_all(&value.to_le_bytes()) {
                self.err = Some(e);
            }
        }
    }

    fn write_raw_bytes(&mut self, bytes: &[u8]) {
        if self.err.is_none() {
            if let Err(e) = self.inner.write_all(bytes) {
                self.err = Some(e);
            }
        }
    }
}

pub struct StructureState<'a, CTX> {
    pub schema_list: FxHashMap<usize, (SchemaId, &'static SchemaRef)>,
    pub span_value: &'a dyn Fn(SpanArgs, &mut StructureState<'_, CTX>, &mut dyn Write),
    pub def_path: &'a dyn Fn(u32, u32, &mut StructureState<'_, CTX>, &mut dyn Write),
    pub crate_num: &'a dyn Fn(u32, &mut StructureState<'_, CTX>, &mut dyn Write),
    pub _marker: PhantomData<&'a CTX>,
}

impl<'a, CTX> StructureState<'a, CTX> {
    pub fn intern_schema(&mut self, schema: &'static SchemaRef) -> SchemaId {
        let key = schema as *const SchemaRef as usize;
        if let Some(id) = self.schema_list.get(&key) {
            return id.0;
        }

        let id = SchemaId(self.schema_list.len() as u32);
        self.schema_list.insert(key, (id, schema));
        id
    }

    pub fn write_schema_header(&mut self, schema: &'static SchemaRef, writer: &mut impl Write) {
        write_tag(writer, ValueKind::Schema as u8);
        let id = self.intern_schema(schema);
        writer.write_raw_u128(id.0.into());
    }
}
