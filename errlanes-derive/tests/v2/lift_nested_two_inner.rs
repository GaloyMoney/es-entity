enum Outer {
    A(Mid),
}

enum Mid {
    B(u8),
    Y(u8),
}

#[derive(errlanes::Lift)]
#[lift(Outer, unhandled = fatal)]
enum Destination {
    #[lift(Outer::A(Mid::B, Mid::Y))]
    B(u8),
}

fn main() {}
