// `narrow_denied` on a `Fault` without a `Denied` lane.
use errlanes::{Fault, ResultExt, lanes};

fn f(r: Result<(), Fault<lanes!(Transient, Fatal)>>) {
    let _ = r.narrow_denied();
}

fn main() {
    f(Ok(()));
}
