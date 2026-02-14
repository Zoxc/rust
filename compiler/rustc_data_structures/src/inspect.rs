//! Lightweight inspection types for debugging and diagnostics.
//!
//! This module provides a `Value` enum that can represent Rust-like
//! data (enums/structs/tuple variants) as well as simple scalar values.
//! It is intentionally small and serde-free; it mirrors the existing
//! use of `rmpv::Value` across this crate but adds structured
//! variants for Rust ADTs.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// A compact representation of values for inspection purposes.
///
/// Models scalars and Rust aggregate types: `Struct`, `StructTuple` and
/// `Enum` (with `EnumVariant`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    Tuple(Vec<Value>),
    /// Map of key -> value.
    Map(Vec<(Value, Value)>),

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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
