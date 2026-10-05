#[derive(Debug, errlanes::Rejection)]
enum Left {
    Shared,
}
#[derive(Debug, errlanes::Rejection)]
enum Right {
    Shared,
}
#[derive(Debug, errlanes::Rejection)]
enum Child {
    One(u32),
    Two,
}

#[errlanes::compose(Left, Right)]
enum AcrossSources {}

#[errlanes::compose(Left)]
enum WithLocalVariant {
    Shared,
}

#[errlanes::compose(Child as Child)]
enum PrefixedWithLocalVariant {
    ChildOne,
}
fn main() {}
