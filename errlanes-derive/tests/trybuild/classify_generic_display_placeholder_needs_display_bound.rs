// Bugbot finding on PR #256: `classify_bounds` only ever added `Debug +
// Send + Sync + 'static` to the generated `Classify` impl. A *non*-debug
// placeholder (`{0}`, not `{0:?}`) over a field whose type is a bare
// generic parameter makes the sibling `Display`/`Error` impl require
// `Display` (via `extra_bounds`) instead of `Debug` -- and `Classify:
// Error` needs that same `Display` bound to be in scope, which
// `classify_bounds` alone never supplied. `emit`'s returned `extra` bounds
// must be merged into the `Classify` impl too.
// No bounds declared on `E` at all -- the point of this test. Before the
// fix, this failed to compile: `classify_bounds` supplied `E: Debug + Send
// + Sync + 'static` but not the `E: Display` the non-debug `{0}`
// placeholder's own `Display`/`Error` impl needed.
#[derive(Debug, errlanes::Classify)]
pub enum DisplayOnly<E> {
    #[classify(fatal(Invariant))]
    #[error("failed: {0}")]
    Failed(E),
}

fn main() {
    assert_eq!(DisplayOnly::Failed("boom").to_string(), "failed: boom");
}
