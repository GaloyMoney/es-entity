#[derive(Debug, errlanes::Rejection)]
enum Source {
    Value { a: u8 },
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "LEAF")]
#[error("leaf")]
struct Leaf;

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "BOUND_LEAF")]
#[error("bound leaf")]
struct BoundLeaf {
    a: u8,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source)]
enum Dropped {
    #[lift(Source::Value { .. })]
    #[error("{0}")]
    Value(Leaf),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source)]
enum Bound {
    #[lift(Source::Value { a, .. })]
    #[rejection(level = "warn")]
    #[error("{0}")]
    Value(BoundLeaf),
}

fn main() {}
