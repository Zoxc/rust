// Copyright 2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![crate_type = "lib"]

#![feature(notail_when_called)]
#![feature(asm)]

// CHECK: define void @foo()
#[no_mangle]
#[notail_when_called]
#[inline(never)]
pub fn foo() {
    unsafe { asm!(""::::"volatile") };
    // CHECK: ret void
}

// CHECK: define void @bar()
#[no_mangle]
#[inline(never)]
pub fn bar() {
    foo();
    // CHECK: notail call {{.+}}
    // CHECK-NEXT: ret void
}
