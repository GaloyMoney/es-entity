use errlanes::{Fault, lanes};
fn main() {
    let _: Option<Fault<lanes!(Transient, Retryable)>> = None;
}
