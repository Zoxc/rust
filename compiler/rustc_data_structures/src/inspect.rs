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
        // Best-effort conversion from `rmpv::Value` into our compact `Value`.
        // If the value cannot be represented exactly, fall back to a
        // readable string representation.
        match Value::try_from_rmpv(v) {
            Ok(val) => val,
            Err(v) => Value::String(Cow::Owned(format!("{:?}", v))),
        }
    }
}

impl Value {
    /// Create a string Value from a &'static str without allocation.
    pub fn from_static_str(s: &'static str) -> Self {
        Value::String(Cow::Borrowed(s))
    }
}

impl Value {
    /// Convert this `Value` into an `rmpv::Value`.
    pub fn into_rmpv(self) -> rmpv::Value {
        use rmpv::Value as V;
        match self {
            Value::Bool(b) => V::from(b),
            Value::Int(i) => {
                if let Ok(i64v) = i64::try_from(i) {
                    V::from(i64v)
                } else {
                    V::Binary(i.to_le_bytes().to_vec())
                }
            }
            Value::UInt(u) => {
                if let Ok(u64v) = u64::try_from(u) {
                    V::from(u64v)
                } else {
                    V::Binary(u.to_le_bytes().to_vec())
                }
            }
            Value::F64(f) => V::from(f),
            Value::Binary(b) => V::Binary(b),
            Value::String(s) => V::from(s.into_owned()),
            Value::Array(a) => V::Array(a.into_iter().map(Value::into_rmpv).collect()),
            Value::Tuple(a) => V::Array(a.into_iter().map(Value::into_rmpv).collect()),
            Value::Map(m) => {
                V::Map(m.into_iter().map(|(k, v)| (k.into_rmpv(), v.into_rmpv())).collect())
            }
            Value::Struct { path, fields } => {
                let fields_map = rmpv::Value::Map(
                    fields
                        .into_iter()
                        .map(|(k, v)| (rmpv::Value::from(k.into_owned()), v.into_rmpv()))
                        .collect(),
                );
                let mut map = Vec::new();
                map.push((rmpv::Value::from("path"), rmpv::Value::from(path.into_owned())));
                map.push((rmpv::Value::from("fields"), fields_map));
                V::Map(map)
            }
            Value::StructTuple { path, fields } => {
                let fields_arr =
                    rmpv::Value::Array(fields.into_iter().map(Value::into_rmpv).collect());
                let mut map = Vec::new();
                map.push((rmpv::Value::from("path"), rmpv::Value::from(path.into_owned())));
                map.push((rmpv::Value::from("fields"), fields_arr));
                V::Map(map)
            }
            Value::Enum { path, variant } => {
                let variant_map = match variant {
                    EnumVariant::Unit(name) => {
                        let mut vm = Vec::new();
                        vm.push((rmpv::Value::from("kind"), rmpv::Value::from("Unit")));
                        vm.push((rmpv::Value::from("name"), rmpv::Value::from(name.into_owned())));
                        rmpv::Value::Map(vm)
                    }
                    EnumVariant::Named(name, fields) => {
                        let fields_map = rmpv::Value::Map(
                            fields
                                .into_iter()
                                .map(|(k, v)| (rmpv::Value::from(k.into_owned()), v.into_rmpv()))
                                .collect(),
                        );
                        let mut vm = Vec::new();
                        vm.push((rmpv::Value::from("kind"), rmpv::Value::from("Named")));
                        vm.push((rmpv::Value::from("name"), rmpv::Value::from(name.into_owned())));
                        vm.push((rmpv::Value::from("fields"), fields_map));
                        rmpv::Value::Map(vm)
                    }
                    EnumVariant::Tuple(name, fields) => {
                        let fields_arr =
                            rmpv::Value::Array(fields.into_iter().map(Value::into_rmpv).collect());
                        let mut vm = Vec::new();
                        vm.push((rmpv::Value::from("kind"), rmpv::Value::from("Tuple")));
                        vm.push((rmpv::Value::from("name"), rmpv::Value::from(name.into_owned())));
                        vm.push((rmpv::Value::from("fields"), fields_arr));
                        rmpv::Value::Map(vm)
                    }
                };
                let mut map = Vec::new();
                map.push((rmpv::Value::from("path"), rmpv::Value::from(path.into_owned())));
                map.push((rmpv::Value::from("variant"), variant_map));
                V::Map(map)
            }
        }
    }

    /// Try to convert an `rmpv::Value` into our `Value`.
    ///
    /// Returns `Ok(Value)` on success, or returns the original `rmpv::Value`
    /// in `Err` when the conversion cannot be performed.
    pub fn try_from_rmpv(v: rmpv::Value) -> Result<Self, rmpv::Value> {
        // Helper to convert arrays/maps recursively.
        if let Some(b) = v.as_bool() {
            return Ok(Value::Bool(b));
        }
        if let Some(i) = v.as_i64() {
            return Ok(Value::Int(i as i128));
        }
        if let Some(u) = v.as_u64() {
            return Ok(Value::UInt(u as u128));
        }
        if let Some(f) = v.as_f64() {
            return Ok(Value::F64(f));
        }
        if let Some(s) = v.as_str() {
            return Ok(Value::String(Cow::Owned(s.to_string())));
        }
        if let Some(bin) = v.as_slice() {
            return Ok(Value::Binary(bin.to_vec()));
        }
        if let Some(arr) = v.as_array() {
            let mut out = Vec::with_capacity(arr.len());
            for e in arr.iter().cloned() {
                out.push(Value::try_from_rmpv(e)?);
            }
            return Ok(Value::Array(out));
        }
        if let Some(map) = v.as_map() {
            // Try to detect our structured encodings first: Struct/StructTuple/Enum
            // Look up helper keys as strings when possible.
            let mut simple_map: Vec<(rmpv::Value, rmpv::Value)> = map.clone();

            // Convert a map of string->value into our fields vector if possible.
            let fn_map_to_fields = |m: &rmpv::Value| -> Option<Vec<(Cow<'static, str>, Value)>> {
                if let Some(mv) = m.as_map() {
                    let mut out = Vec::with_capacity(mv.len());
                    for (k, v) in mv.iter() {
                        if let Some(ks) = k.as_str() {
                            let val = Value::try_from_rmpv(v.clone()).ok()?;
                            out.push((Cow::Owned(ks.to_string()), val));
                        } else {
                            return None;
                        }
                    }
                    return Some(out);
                }
                None
            };

            // Helper to find a string-valued entry by key name
            let find_key = |name: &str| -> Option<rmpv::Value> {
                for (k, v) in simple_map.iter() {
                    if let Some(ks) = k.as_str() {
                        if ks == name {
                            return Some(v.clone());
                        }
                    }
                }
                None
            };

            if let Some(path_v) = find_key("path") {
                // Structured types
                if let Some(variant_v) = find_key("variant") {
                    // Enum
                    if let Some(path_s) = path_v.as_str() {
                        if let Some(variant_map) = variant_v.as_map() {
                            // read kind and name
                            let mut kind = None::<&str>;
                            let mut name = None::<&str>;
                            let mut fields_val: Option<rmpv::Value> = None;
                            for (kk, vv) in variant_map.iter() {
                                if let Some(kk_s) = kk.as_str() {
                                    match kk_s {
                                        "kind" => kind = vv.as_str(),
                                        "name" => name = vv.as_str(),
                                        "fields" => fields_val = Some(vv.clone()),
                                        _ => {}
                                    }
                                }
                            }
                            if let Some(k) = kind {
                                if let Some(n) = name {
                                    let path = Cow::Owned(path_s.to_string());
                                    match k {
                                        "Unit" => {
                                            return Ok(Value::Enum {
                                                path,
                                                variant: EnumVariant::Unit(Cow::Owned(
                                                    n.to_string(),
                                                )),
                                            });
                                        }
                                        "Named" => {
                                            if let Some(fv) = fields_val {
                                                if let Some(fields) = fn_map_to_fields(&fv) {
                                                    return Ok(Value::Enum {
                                                        path,
                                                        variant: EnumVariant::Named(
                                                            Cow::Owned(n.to_string()),
                                                            fields,
                                                        ),
                                                    });
                                                }
                                            }
                                        }
                                        "Tuple" => {
                                            if let Some(fv) = fields_val {
                                                if let Some(arr) = fv.as_array() {
                                                    let mut out = Vec::with_capacity(arr.len());
                                                    for e in arr.iter().cloned() {
                                                        out.push(Value::try_from_rmpv(e)?);
                                                    }
                                                    return Ok(Value::Enum {
                                                        path,
                                                        variant: EnumVariant::Tuple(
                                                            Cow::Owned(n.to_string()),
                                                            out,
                                                        ),
                                                    });
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                } else if let Some(fields_v) = find_key("fields") {
                    // Struct or StructTuple
                    if let Some(path_s) = path_v.as_str() {
                        let path = Cow::Owned(path_s.to_string());
                        if let Some(fm) = fields_v.as_map() {
                            // named fields
                            let mut out_fields = Vec::with_capacity(fm.len());
                            for (k, v) in fm.iter() {
                                if let Some(ks) = k.as_str() {
                                    out_fields.push((
                                        Cow::Owned(ks.to_string()),
                                        Value::try_from_rmpv(v.clone())?,
                                    ));
                                } else {
                                    return Err(v.clone());
                                }
                            }
                            return Ok(Value::Struct { path, fields: out_fields });
                        }
                        if let Some(arr) = fields_v.as_array() {
                            let mut out = Vec::with_capacity(arr.len());
                            for e in arr.iter().cloned() {
                                out.push(Value::try_from_rmpv(e)?);
                            }
                            return Ok(Value::StructTuple { path, fields: out });
                        }
                    }
                }
            }

            // Fallback: interpret as a generic map where keys and values are converted.
            let mut out = Vec::with_capacity(map.len());
            for (k, v) in map.iter().cloned() {
                let k2 = Value::try_from_rmpv(k)?;
                let v2 = Value::try_from_rmpv(v)?;
                out.push((k2, v2));
            }
            return Ok(Value::Map(out));
        }

        // Unknown or unsupported representation: return the original value as Err
        Err(v)
    }
}
