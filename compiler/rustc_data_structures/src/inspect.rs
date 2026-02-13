//! Lightweight inspection types for debugging and diagnostics.
//!
//! This module provides a `Value` enum that can represent Rust-like
//! data (enums/structs/tuple variants) as well as simple scalar values.
//! It is intentionally small and serde-free; it mirrors the existing
//! use of `rmpv::Value` across this crate but adds structured
//! variants for Rust ADTs.

use crate::stable_hasher::rmpv;
use std::borrow::Cow;

/// A compact representation of values for inspection purposes.
///
/// Models scalars and Rust aggregate types: `Struct`, `StructTuple` and
/// `Enum` (with `EnumVariant`).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Boolean.
    Bool(bool),
    /// Integer (signed).
    Int(i128),
    /// Unsigned integer.
    UInt(u128),
    /// Floating point.
    F64(f64),
    /// Binary blob.
    Binary(Vec<u8>),
    /// String-like value.
    String(Cow<'static, str>),
    /// Array of values.
    Array(Vec<Value>),
    /// Map of key -> value.
    Map(Vec<(Value, Value)>),

    /// Named-field struct value.
    Struct { path: Cow<'static, str>, fields: Vec<(Cow<'static, str>, Value)> },

    /// Tuple struct / tuple variant value.
    StructTuple { path: Cow<'static, str>, fields: Vec<Value> },

    /// Enum value: `path` is the enum path and `variant` describes the
    /// active variant.
    Enum { path: Cow<'static, str>, variant: EnumVariant },
}

/// Describes a single enum variant instance.
#[derive(Clone, Debug, PartialEq)]
pub enum EnumVariant {
    /// Unit variant (no fields).
    Unit(Cow<'static, str>),
    /// Named fields (struct-like variant).
    Named(Cow<'static, str>, Vec<(Cow<'static, str>, Value)>),
    /// Positional fields (tuple-like variant).
    Tuple(Cow<'static, str>, Vec<Value>),
}

impl From<rmpv::Value> for Value {
    fn from(v: rmpv::Value) -> Self {
        use rmpv::Value as V;
        match v {
            V::Nil => Value::Nil,
            V::Boolean(b) => Value::Bool(b),
            V::Integer(i) => Value::Rmpv(V::Integer(i)),
            V::F32(f) => Value::F64(f as f64),
            V::F64(f) => Value::F64(f),
            V::String(s) => Value::String(Cow::Owned(s.to_string())),
            V::Binary(b) => Value::Binary(b),
            V::Array(a) => Value::Array(a.into_iter().map(Value::from).collect()),
            V::Map(m) => {
                Value::Map(m.into_iter().map(|(k, v)| (Value::from(k), Value::from(v))).collect())
            }
            other => Value::Rmpv(other),
        }
    }
}

impl Value {
    /// Create a string Value from a &'static str without allocation.
    pub fn from_static_str(s: &'static str) -> Self {
        Value::String(Cow::Borrowed(s))
    }
}
