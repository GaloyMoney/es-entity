use errlanes::{Lift, Rejection};

#[derive(Debug, errlanes::Rejection)]
enum Source {
    #[rejection(code = "SOURCE_VALUE", level = "warn")]
    Value(String),
}

// Rejection may read the mapping for metadata, but must not implement Lift/From.
#[derive(Debug, errlanes::Rejection)]
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

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source)]
enum Forwarded {
    #[lift(Source::Value)]
    Renamed(String),
}

// Both derive orderings work: neither relies on the other's expansion.
#[derive(Debug, errlanes::Lift, errlanes::Rejection)]
#[lift(Source)]
enum Reinterpreted {
    #[lift(Source::Value)]
    #[rejection(code = "LOCAL_VALUE", level = "debug")]
    Renamed(String),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source)]
enum CodeOverride {
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

#[derive(Debug, errlanes::Rejection)]
enum ExplicitForward {
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

// A whole-value arm: `Payload` is a struct source (not an enum variant), so
// `#[lift(Payload)]` with no variant suffix wraps the entire value. Lift
// sources are rejections, so `Payload` derives one.
#[derive(Debug, PartialEq, errlanes::Rejection)]
#[rejection(code = "INVALID_PAYLOAD")]
struct Payload(u32);

#[derive(Debug, PartialEq, errlanes::Lift)]
#[lift(Payload)]
enum JobRejection {
    #[lift(Payload)]
    InvalidPayload(Payload),
}

#[test]
fn whole_value_lift_wraps_a_struct_source_directly() {
    assert_eq!(
        JobRejection::from(Payload(7)),
        JobRejection::InvalidPayload(Payload(7))
    );
    assert_eq!(
        JobRejection::lift(Payload(7)),
        Ok(JobRejection::InvalidPayload(Payload(7)))
    );
}
