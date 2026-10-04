// Finding 1 of the `errlanes-classify-derive-codegen-gaps` handoff: a
// type-level lane (`#[classify(fatal(..))]` on the enum itself) used to
// build a single `Unit` for the whole enum, so every variant's own
// `#[error(..)]` was silently discarded and every variant rendered the same
// snake_cased *type* name. Each variant must render its own message.
#[derive(Debug, errlanes::Classify)]
#[classify(fatal(Invariant))]
enum ExpectEvent {
    #[error("timeout waiting for event")]
    Timeout,
    #[error("trigger failed: {0:?}")]
    TriggerFailed(String),
}

fn main() {
    assert_eq!(ExpectEvent::Timeout.to_string(), "timeout waiting for event");
    assert_eq!(
        ExpectEvent::TriggerFailed("boom".to_string()).to_string(),
        "trigger failed: \"boom\""
    );
    // The two variants must render *differently* -- the bug collapsed both
    // to the same snake_cased type name (`expect_event`).
    assert_ne!(
        ExpectEvent::Timeout.to_string(),
        ExpectEvent::TriggerFailed("boom".to_string()).to_string()
    );
}
