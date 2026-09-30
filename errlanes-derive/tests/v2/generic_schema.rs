#![allow(unused_imports)]
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Generic<T: std::fmt::Debug + std::fmt::Display> { #[error("{0}")] Value(T) }
fn main() {}
