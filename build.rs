//! Re-run the `EsRepo` derives whenever a migration changes.
//!
//! The derive parses the committed migration `.sql` files at expansion time to
//! build the physical index catalog that drives `list_for_filters`
//! specialization. Proc-macro output is not otherwise invalidated by external
//! file edits, so this `rerun-if-changed` on the migrations directory keeps the
//! generated code in sync (cargo scans the directory, so it fires on file
//! additions too — no `cargo clean` needed).
//!
//! Downstream crates whose migrations live outside their own manifest directory
//! (e.g. a `core/*` crate reading `../../app/migrations`) should emit
//! `cargo:rustc-env=ES_ENTITY_MIGRATIONS_DIR=<abs>` alongside this
//! `rerun-if-changed`; that env var takes precedence over the default
//! `$CARGO_MANIFEST_DIR/migrations` resolution.

fn main() {
    println!("cargo:rerun-if-env-changed=ES_ENTITY_MIGRATIONS_DIR");

    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by cargo");
    let migrations = std::path::Path::new(&manifest_dir).join("migrations");
    if migrations.is_dir() {
        println!("cargo:rerun-if-changed={}", migrations.display());
    }
}
