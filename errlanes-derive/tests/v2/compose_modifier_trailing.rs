#[derive(Debug, errlanes::Rejection)]
enum Leaf {
    Disabled,
}

#[errlanes::compose(Leaf, no_from)]
#[derive(Debug)]
enum Trailing {}

#[errlanes::compose(Leaf, no_leaf_from)]
#[derive(Debug)]
enum TrailingLeaf {}

fn main() {}
