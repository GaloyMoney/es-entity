#[derive(Debug, errlanes::Rejection)]
enum Left { Shared }
#[derive(Debug, errlanes::Rejection)]
enum Right { Shared }

#[errlanes::compose(union(Left, Right))]
enum Unknown {
    #[compose(merge(Left::Missing))]
    #[rejection(code = "CANONICAL")]
    Shared,
}

#[errlanes::compose(union(Left))]
enum Unlisted {
    #[compose(merge(Right::Shared))]
    #[rejection(code = "CANONICAL")]
    Shared,
}

#[errlanes::compose(union(Left))]
enum Duplicate {
    #[compose(merge(Left::Shared, Left::Shared))]
    #[rejection(code = "CANONICAL")]
    Shared,
}

#[errlanes::compose(union(Left))]
enum Overlap {
    #[compose(merge)]
    #[rejection(code = "CANONICAL")]
    Shared,
    #[compose(merge(Left::Shared))]
    #[rejection(code = "OTHER")]
    Other,
}

#[errlanes::compose(union(Left))]
enum NoMatch {
    #[compose(merge)]
    #[rejection(code = "CANONICAL")]
    Missing,
}
fn main() {}
