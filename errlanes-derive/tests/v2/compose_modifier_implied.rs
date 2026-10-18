#[derive(Debug, errlanes::Rejection)]
enum Leaf {
    Disabled,
}

#[errlanes::compose(Leaf(no_from, no_leaf_from))]
#[derive(Debug)]
enum Top {}

fn main() {}
