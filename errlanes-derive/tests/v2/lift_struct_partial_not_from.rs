#[derive(Debug, errlanes::Rejection)]
enum Source { Value { value: u32 }, Other }

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[rejection(code = "DESTINATION")]
#[lift(Source, variant = Value, unhandled = fatal)]
struct Destination { value: u32 }

fn main() {
    let _: Destination = Source::Value { value: 1 }.into();
}
