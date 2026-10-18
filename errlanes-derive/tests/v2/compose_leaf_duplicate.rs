#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "LEAF")]
struct Leaf(u8);

#[derive(Debug, errlanes::Rejection)]
enum Accounts {
    #[rejection(delegate, from)]
    Member(Leaf),
}

#[derive(Debug, errlanes::Rejection)]
enum Sets {
    #[rejection(delegate, from)]
    Set(Leaf),
}

#[errlanes::compose(Accounts, Sets as Other)]
#[derive(Debug)]
enum Composed {}

fn main() {}
