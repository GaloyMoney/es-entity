#[derive(Debug, errlanes::Rejection)]
enum Source {
    #[rejection(code = "SX")]
    X(u32),
}

#[derive(Debug, errlanes::Rejection)]
enum Dest {
    #[rejection(code_and_level_from = Source::X, description = "nope")]
    OnlyX(u32),
}

fn main() {}
