//! Classifies `reqwest` 0.13 errors using the same lane policy as
//! `reqwest` (the independently enabled 0.12 adapter).
//!
//! Upstream service-account authorization failures usually need
//! `#[classify(delegate, narrow(Denied))]` at the consuming boundary.

use ::reqwest_013 as client;

include!("reqwest_impl.rs");
