struct Payload {
    value: u8,
}

#[derive(errlanes::Lift)]
#[lift(Payload, unhandled = fatal)]
enum Destination {
    #[lift(Payload(Payload))]
    Whole(Payload),
}

fn main() {}
