#[derive(errlanes::Lift)]
struct Unit;

#[derive(errlanes::Lift)]
struct Tuple(u32);

#[derive(errlanes::Lift)]
union Union { value: u32 }

fn main() {}
