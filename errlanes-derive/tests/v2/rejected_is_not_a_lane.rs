use errlanes::{Fault, lanes};
fn main() {
    let _: Option<Fault<lanes!(Rejected, Fatal)>> = None;
}
