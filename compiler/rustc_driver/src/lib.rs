//! A crate which just re-exports all of `rustc_driver_impl` as a dylib.
//! This allows `rustc_driver_impl` to be compiled in parallel with other crates.

#![allow(unused_extern_crates)]
extern crate rustc_driver_impl;

pub use rustc_driver_impl::*;
