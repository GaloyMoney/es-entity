enum Source { Value { left: u32, right: u32 } }

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Value)]
    Value {
        #[lift(from = right)]
        left: u32,
        #[lift(from = left)]
        right: u32,
    },
}

fn main() {}
