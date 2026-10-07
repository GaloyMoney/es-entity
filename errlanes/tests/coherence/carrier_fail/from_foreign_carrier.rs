// KNOWN LIMITATION, pinned: `from(..)` cannot list a carrier declared in
// another crate. The carrier's blanket inbound `From<W: IntoLanes<Kind =
// Plain>>` and `From<upstream::HostFault>` overlap, because
// `upstream::HostFault: Classify` is unknowable from here.
#[derive(Debug, errlanes::Carrier)]
#[carrier(from(upstream::HostFault))]
enum PartyFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn main() {}
