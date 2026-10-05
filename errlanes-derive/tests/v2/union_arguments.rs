#[derive(Debug, errlanes::Rejection)]
enum Source { Shared }

#[errlanes::compose(union())]
enum Empty {}

#[errlanes::compose(union(Source, Source))]
enum Duplicate {}

#[errlanes::compose(union(Source<u32>))]
enum Generic {}

#[errlanes::compose(union(Source))]
#[lift(Source)]
enum AlreadyMapped {}

#[errlanes::compose(union(Source))]
enum EmptyMerge {
    #[compose(merge())]
    #[rejection(code = "SHARED")]
    Shared,
}

#[errlanes::compose(union(Source))]
enum DuplicateMerge {
    #[compose(merge)]
    #[compose(merge(Source::Shared))]
    #[rejection(code = "SHARED")]
    Shared,
}
fn main() {}
