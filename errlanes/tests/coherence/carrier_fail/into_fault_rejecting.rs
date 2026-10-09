use errlanes::ResultExt;

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "REJECTED")]
struct Rejected;

fn main() {
    let _ = Err::<(), _>(Rejected).into_fault();
}
