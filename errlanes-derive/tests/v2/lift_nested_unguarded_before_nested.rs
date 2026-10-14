enum Outer {
    A(Mid),
}

enum Mid {
    B(u8),
}

#[derive(errlanes::Lift)]
#[lift(Outer, unhandled = fatal)]
enum Destination {
    #[lift(Outer::A)]
    Any(Mid),
    #[lift(Outer::A(Mid::B))]
    B(u8),
}

fn main() {}
