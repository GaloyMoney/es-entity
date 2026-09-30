#![allow(unused_imports)]
#[derive(Debug, Clone)] struct Custom;
impl errlanes::LaneProfile for Custom {
    type Denied = errlanes::Denied;
    type Transient = errlanes::Transient;
    type Fatal = errlanes::Fatal;
}
fn main() {}
