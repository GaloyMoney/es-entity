#[derive(Debug, errlanes::Rejection)]
enum Left { Shared }
#[derive(Debug, errlanes::Rejection)]
enum Right { Shared }

#[errlanes::compose(union(Left, Right))]
enum Collision {}

#[errlanes::compose(union(Left))]
enum LocalCollision { Shared }
fn main() {}
