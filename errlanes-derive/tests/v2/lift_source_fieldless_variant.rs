// A whole-value `#[lift(Source)]` names a registered struct source directly;
// the destination variant must have a field to hold it. A unit variant
// cannot, and must be a spanned error, not a panic during macro expansion.
#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "INVALID_PAYLOAD")]
struct Payload(#[source] std::num::ParseIntError);

#[derive(Debug, errlanes::Rejection)]
#[lift(Payload)]
enum JobRejection {
    #[lift(Payload)]
    InvalidPayload,
}

fn main() {}
