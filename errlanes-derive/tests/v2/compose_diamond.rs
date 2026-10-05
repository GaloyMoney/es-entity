#[derive(Debug, errlanes::Rejection)]
pub enum Leaf {
    One,
}
#[errlanes::compose(Leaf as L)]
#[derive(Debug)]
pub enum Left {}
#[errlanes::compose(Leaf as R)]
#[derive(Debug)]
pub enum Right {}

#[errlanes::compose(Left, Right)]
enum Bare {}

#[errlanes::compose(Left as A, Right as B)]
enum Prefixed {}
fn main() {}
