#[derive(Debug, errlanes::Rejection)]
enum Leaf {
    Disabled,
}

#[errlanes::compose(Leaf, no_from)]
#[derive(Debug)]
enum Trailing {}

fn main() {}
