#[derive(Debug, errlanes::Rejection)]
enum Left { Shared(u32) }
#[derive(Debug, errlanes::Rejection)]
enum Right { Shared(String) }

#[errlanes::compose(Left, Right)]
#[derive(Debug)]
enum WrongType {
    #[compose(merge)]
    #[rejection(code = "CANONICAL")]
    Shared(u32),
}

#[errlanes::compose(Left)]
#[derive(Debug)]
enum WrongShape {
    #[compose(merge)]
    #[rejection(code = "CANONICAL")]
    Shared { value: u32 },
}
fn main() {}
