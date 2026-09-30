// `#[errlanes::instrument]` does not require its return type's error to be
// `Laned` up front; the compiler catches it at the generated `record` call
// instead, the same nudge any other `E: Laned` bound would give.
#[derive(Debug)]
struct NotLaned;

impl std::fmt::Display for NotLaned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not laned")
    }
}
impl std::error::Error for NotLaned {}

#[errlanes::instrument]
fn step() -> Result<(), NotLaned> {
    Err(NotLaned)
}

fn main() {}
