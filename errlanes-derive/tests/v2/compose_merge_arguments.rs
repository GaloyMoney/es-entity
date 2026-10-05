#[derive(Debug, errlanes::Rejection)]
enum Source {
    Shared,
}

#[errlanes::compose(Source)]
enum EmptyMerge {
    #[compose(merge())]
    #[rejection(code = "SHARED")]
    Shared,
}

#[errlanes::compose(Source)]
enum DuplicateMerge {
    #[compose(merge)]
    #[compose(merge(Source::Shared))]
    #[rejection(code = "SHARED")]
    Shared,
}
fn main() {}
