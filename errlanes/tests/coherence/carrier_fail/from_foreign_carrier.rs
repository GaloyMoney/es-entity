// KNOWN LIMITATION, pinned: `from(..)` cannot list a carrier declared in
// another crate. The carrier's blanket inbound `From<W: IntoLanes<Kind =
// Plain>>` and `From<upstream::HostFault>` overlap, because
// `upstream::HostFault: Classify` is unknowable from here.
#[errlanes::fault(Transient, Fatal; from(upstream::HostFault))]
struct PartyFault;

fn main() {}
