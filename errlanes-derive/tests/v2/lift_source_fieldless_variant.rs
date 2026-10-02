// A whole-value `#[lift(Source)]` names a registered struct source directly;
// the destination variant must have a field to hold it. A unit variant
// cannot, and must be a spanned error, not a panic during macro expansion.
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[error("invalid payload")]
#[rejection(code = "INVALID_PAYLOAD")]
struct Payload(#[source] std::num::ParseIntError);

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[lift(Payload)]
enum JobRejection {
    #[error("invalid payload")]
    #[lift(Payload)]
    InvalidPayload,
}

fn main() {}
