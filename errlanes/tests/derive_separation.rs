use errlanes::{Lift, Rejection};

// Conversion-only enums need neither Error nor Rejection, including their payloads.
#[derive(Debug, PartialEq)]
enum Input<T> {
    Value(T),
}

#[derive(Debug, PartialEq, errlanes::Lift)]
#[lift(Input::<T>)]
enum Output<T> {
    #[lift(Input::<T>::Value)]
    Renamed(T),
}

#[test]
fn lift_works_without_rejection_or_error() {
    assert_eq!(
        Output::from(Input::Value(vec![1, 2])),
        Output::Renamed(vec![1, 2])
    );
    assert_eq!(Output::lift(Input::Value(42)), Ok(Output::Renamed(42)));
}

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Source {
    #[error("source {0}")]
    #[rejection(code = "SOURCE_VALUE", level = "warn")]
    Value(String),
}

// Rejection may read the mapping for metadata, but must not implement Lift/From.
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[lift(Source)]
enum MetadataOnly {
    #[error("destination {0}")]
    #[lift(Source::Value)]
    Renamed(String),
}

impl From<Source> for MetadataOnly {
    fn from(source: Source) -> Self {
        let Source::Value(value) = source;
        Self::Renamed(value)
    }
}

#[test]
fn rejection_does_not_generate_conversions() {
    let rejection = MetadataOnly::lift(Source::Value("payload".into())).unwrap();
    assert_eq!(rejection.code().to_string(), "SOURCE_VALUE");
    assert_eq!(rejection.level(), errlanes::Level::Warn);
    assert_eq!(rejection.to_string(), "destination payload");
}

#[derive(Debug, thiserror::Error, errlanes::Rejection, errlanes::Lift)]
#[lift(Source)]
enum Forwarded {
    #[error("forwarded {0}")]
    #[lift(Source::Value)]
    Renamed(String),
}

// Both derive orderings work: neither relies on the other's expansion.
#[derive(Debug, thiserror::Error, errlanes::Lift, errlanes::Rejection)]
#[lift(Source)]
enum Reinterpreted {
    #[error("local {0}")]
    #[lift(Source::Value)]
    #[rejection(code = "LOCAL_VALUE", level = "debug")]
    Renamed(String),
}

#[derive(Debug, thiserror::Error, errlanes::Rejection, errlanes::Lift)]
#[lift(Source)]
enum CodeOverride {
    #[error("local {0}")]
    #[lift(Source::Value)]
    #[rejection(code = "LOCAL_VALUE")]
    Renamed(String),
}

#[test]
fn simple_lift_forwards_metadata_unless_overridden() {
    let rejection = Forwarded::from(Source::Value("payload".into()));
    assert_eq!(rejection.code().to_string(), "SOURCE_VALUE");
    assert_eq!(rejection.level(), errlanes::Level::Warn);

    let rejection = Reinterpreted::from(Source::Value("payload".into()));
    assert_eq!(rejection.code().to_string(), "LOCAL_VALUE");
    assert_eq!(rejection.level(), errlanes::Level::Debug);

    let rejection = CodeOverride::from(Source::Value("payload".into()));
    assert_eq!(rejection.code().to_string(), "LOCAL_VALUE");
    assert_eq!(rejection.level(), errlanes::Level::Info);
}

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum ExplicitForward {
    #[error("explicit {0}")]
    #[rejection(forward = Source::Value)]
    Renamed(String),
}

#[test]
fn explicit_metadata_forwarding_needs_no_lift() {
    let rejection = ExplicitForward::Renamed("payload".into());
    assert_eq!(rejection.code().to_string(), "SOURCE_VALUE");
    assert_eq!(rejection.level(), errlanes::Level::Warn);
}

#[derive(Debug, PartialEq)]
enum PartialInput {
    Accepted(u32),
    Unhandled(String),
}

#[derive(Debug, PartialEq, errlanes::Lift)]
#[lift(PartialInput, unhandled = fatal)]
enum PartialOutput {
    #[lift(PartialInput::Accepted)]
    Accepted(u32),
}

#[test]
fn partial_lift_alone_retains_the_unmapped_input() {
    assert_eq!(
        PartialOutput::lift(PartialInput::Accepted(3)),
        Ok(PartialOutput::Accepted(3))
    );
    assert_eq!(
        PartialOutput::lift(PartialInput::Unhandled("original".into())),
        Err(PartialInput::Unhandled("original".into())),
    );
}
