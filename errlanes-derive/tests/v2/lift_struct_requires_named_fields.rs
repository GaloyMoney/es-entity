#[derive(errlanes::Lift)]
struct Pair(u32, u32);

#[derive(errlanes::Lift)]
struct Tuple(u32);

#[derive(errlanes::Lift)]
union Union { value: u32 }

fn main() {}
