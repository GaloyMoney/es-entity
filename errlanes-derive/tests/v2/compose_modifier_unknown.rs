#[derive(Debug, errlanes::Rejection)]
enum Leaf {
    Disabled,
}

#[errlanes::compose(Leaf(deep))]
#[derive(Debug)]
enum Top {}

fn main() {}
