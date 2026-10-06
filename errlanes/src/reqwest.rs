//! Classifies a raw `reqwest::Error`, exactly once, at the point it is born.
//!
//! **`Denied` means the subject of *this* call is unauthorized.** An
//! upstream 401/403 returned to a service account is usually a
//! credential/configuration fault at our own layer, not the caller's — most
//! consumers narrow it away (`#[classify(delegate, narrow(Denied))]`, which
//! becomes `Fatal(Denied)`) or match the status in a hand-written `with` fn
//! to get `Fatal(Config)` instead. A proxy-style wrapper that calls upstream
//! with the caller's *own* token is the case that keeps `Denied` as-is.
//! Keeping it in the built-in table preserves the information either way;
//! narrowing it is one attribute at the call site.

use ::reqwest as client;

include!("reqwest_impl.rs");
