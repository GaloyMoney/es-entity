#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "SHARED", code_prefix = "P_")]
enum Prefixed {
    A,
    B,
}

fn main() {}
