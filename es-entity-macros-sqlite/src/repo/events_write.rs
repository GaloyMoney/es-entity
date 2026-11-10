//! Snapshot glue shared by the write paths.
//!
//! On Postgres this module owns the combined index+events statement (the
//! `UNNEST` events tail and the `snap` upsert CTE). SQLite has neither, so the
//! events, payload and snapshot writes live in `persist_events` /
//! `es_entity::sqlite_write`; only the Rust-side compaction step is shared.

use proc_macro2::TokenStream;
use quote::quote;

/// Snapshot compaction after a write that upserted a snapshot.
pub struct SnapshotUpsert<'a> {
    #[allow(dead_code)]
    pub table: &'a str,
}

impl SnapshotUpsert<'_> {
    /// Compacts one entity's tail into the snapshot it just wrote (a no-op
    /// when `snapshot()` returned `None`). Must run after the post-persist
    /// hook. Requires `__snapshot` and `__snapshot_first` in scope.
    pub fn compact_per_entity(&self, events: TokenStream, recorded_at: TokenStream) -> TokenStream {
        quote! {
            if let Some(s) = __snapshot {
                #events.compact_to_snapshot(s, #recorded_at, __snapshot_first.unwrap_or(#recorded_at));
            }
        }
    }
}
