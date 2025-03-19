//@ compile-flags: -Z threads=2
//~^ERROR cycle detected when

const FOO: usize = FOO;

fn main() {}
