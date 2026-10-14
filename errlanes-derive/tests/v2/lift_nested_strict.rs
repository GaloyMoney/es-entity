enum Outer {
    A(Mid),
    Z,
}

enum Mid {
    B(u8),
    Y,
}

#[derive(errlanes::Lift)]
#[lift(Outer)]
enum Destination {
    #[lift(Outer::A(Mid::B))]
    B(u8),
}

fn main() {}
