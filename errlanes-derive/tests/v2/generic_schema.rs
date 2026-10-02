#![allow(unused_imports)]
#[derive(Debug, errlanes::Rejection)]
enum Generic<T: std::fmt::Debug + std::fmt::Display> { Value(T) }
fn main() {}
