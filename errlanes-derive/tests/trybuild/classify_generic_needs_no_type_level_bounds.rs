// Finding 2 of the `errlanes-classify-derive-codegen-gaps` handoff: the
// generated `Classify` impl carried no inferred bounds, so `Classify: Error
// + Send + Sync + 'static` had to be satisfied by declaring `Debug + Send +
// Sync + 'static` on the *type* itself -- forcing every signature naming it
// to repeat those bounds. The bare `<E>` below must compile with no bounds
// declared on the enum at all; the generated `Classify` impl carries them
// instead.
#[derive(Debug, errlanes::Classify)]
pub enum ExpectEventError<E> {
    #[classify(fatal(Invariant))]
    #[error("Timeout waiting for event")]
    Timeout,
    #[classify(fatal(Invariant))]
    #[error("Trigger failed: {0:?}")]
    TriggerFailed(E),
}

fn main() {
    use errlanes::Classify;
    assert_eq!(
        ExpectEventError::<String>::Timeout.to_string(),
        "Timeout waiting for event"
    );
    assert_eq!(
        ExpectEventError::TriggerFailed("boom".to_string()).to_string(),
        "Trigger failed: \"boom\""
    );
    let _ = ExpectEventError::<String>::Timeout.classify();
}
