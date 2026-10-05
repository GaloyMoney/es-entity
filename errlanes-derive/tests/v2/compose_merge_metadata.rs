#[derive(Debug, errlanes::Rejection)]
enum Left { Shared }
#[derive(Debug, errlanes::Rejection)]
enum Right { Shared }

#[errlanes::compose(Left, Right)]
enum NoCanonicalMetadata {
    #[compose(merge)]
    Shared,
}

#[errlanes::compose(Left, Right)]
enum LevelOnly {
    #[compose(merge)]
    #[rejection(level = "warn")]
    Shared,
}
fn main() {}
