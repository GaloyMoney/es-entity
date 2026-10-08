#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "LEAF")]
struct Leaf;

#[derive(Debug, errlanes::Rejection)]
enum Family {
    #[rejection(delegate, from, description = "nope")]
    Inner(Leaf),
}

fn main() {}
