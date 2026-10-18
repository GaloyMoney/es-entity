#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "LEAF")]
struct Leaf(u8);

#[derive(Debug, errlanes::Rejection)]
enum Accounts {
    #[rejection(delegate, from)]
    Member(Leaf),
}

#[errlanes::compose(Accounts(no_leaf_from))]
#[derive(Debug)]
enum Composed {}

fn main() {
    let _ = Composed::from(Accounts::Member(Leaf(1)));
    let _ = Composed::from(Leaf(1));
}
