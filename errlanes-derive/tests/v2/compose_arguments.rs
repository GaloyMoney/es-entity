#[errlanes::compose(prefix = "X")]
enum EnumArgs {}

#[errlanes::compose]
enum VariantArgs {
    #[compose(flatten, prefix = "X")]
    Child(Source),
}

#[errlanes::compose]
enum UnknownOption {
    #[compose(rename)]
    Child(Source),
}

#[errlanes::compose]
enum DuplicateOption {
    #[compose(flatten)]
    #[compose(flatten)]
    Child(Source),
}

#[errlanes::compose]
enum LegacyOption {
    #[flatten]
    Child(Source),
}
fn main() {}
