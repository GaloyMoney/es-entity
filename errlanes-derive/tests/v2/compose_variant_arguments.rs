#[derive(Debug, errlanes::Rejection)]
enum Source {
    Shared,
}

#[errlanes::compose(Source)]
enum UnknownOption {
    #[compose(rename)]
    #[rejection(code = "SHARED")]
    Shared,
}

// The placeholder form is gone: a whole family is listed on the attribute.
#[errlanes::compose(Source)]
enum RemovedPlaceholder {
    #[compose(flatten)]
    Child(Source),
}

#[errlanes::compose]
enum MergeWithoutSources {
    #[compose(merge)]
    #[rejection(code = "SHARED")]
    Shared,
}

#[errlanes::compose]
enum LegacyAttribute {
    #[flatten]
    Child(Source),
}
fn main() {}
