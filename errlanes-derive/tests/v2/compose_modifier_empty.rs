#[derive(Debug, errlanes::Rejection)]
enum Leaf {
    Disabled,
}

#[errlanes::compose(Leaf())]
#[derive(Debug)]
enum Top {}

fn main() {}
