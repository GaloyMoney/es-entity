use errlanes::{Fault, ResultExt, lanes};

fn main() {
    let source: Result<(), Fault<lanes!(Fatal)>> = Ok(());
    let _ = source.lift::<()>();
}
