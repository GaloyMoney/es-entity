#[derive(Debug, errlanes::Rejection)]
pub enum Leaf { One }
#[errlanes::compose]
#[derive(Debug)]
pub enum Left { #[compose(flatten)] Leaf(Leaf) }
#[errlanes::compose]
#[derive(Debug)]
pub enum Right { #[compose(flatten)] Leaf(Leaf) }
#[errlanes::compose]
#[derive(Debug)]
pub enum Diamond {
    #[compose(flatten)] Left(Left),
    #[compose(flatten)] Right(Right),
}
fn main() {}
