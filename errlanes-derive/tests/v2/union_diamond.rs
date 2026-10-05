#[derive(Debug, errlanes::Rejection)]
pub enum Leaf { One }
#[errlanes::compose]
#[derive(Debug)]
pub enum Left { #[compose(flatten)] L(Leaf) }
#[errlanes::compose]
#[derive(Debug)]
pub enum Right { #[compose(flatten)] R(Leaf) }
#[errlanes::compose(union(Left, Right))]
enum Diamond {}
fn main() {}
