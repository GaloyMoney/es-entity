//! Classifies a raw `serde_json::Error`, exactly once, at the point it is
//! born.
//!
//! Always `Fatal(Invariant)` for a syntax/data/EOF failure — a syntax/data/EOF
//! failure means the bytes serde was handed don't decode at all, which is a
//! violated invariant (whoever produced them didn't produce valid JSON for
//! this type), not necessarily corrupt storage; a caller wrapping a
//! genuinely storage-specific decode failure (e.g. a persisted event row)
//! pins its own kind instead of delegating. A serde message quotes the
//! input it failed on, which is correct for stored data (an operator needs
//! to see it) but wrong for caller-supplied bytes: wrap those in a local
//! `Rejection` wrapper instead (see the crate README's "Errors from other
//! crates" section) so the message discipline still holds.

use ::serde_json::error::Category;

use crate::{
    fail::{Fail, Fault},
    lane::{Fatal, FatalKind},
};

/// Which lane a `serde_json::Error` belongs to — the one table both public
/// classifiers read.
fn lane_table(e: &::serde_json::Error) -> Fault<crate::lanes!(Fatal)> {
    match e.classify() {
        Category::Io => Fatal::new(FatalKind::Dependency).into(),
        Category::Syntax | Category::Data | Category::Eof => {
            Fatal::new(FatalKind::Invariant).into()
        }
    }
}

/// Classifies a raw `serde_json::Error`, moving it — the error itself
/// becomes the `Fatal`'s source, so the whole chain survives. What
/// `impl Classify for serde_json::Error` delegates to.
fn classify_serde_json_fault(e: ::serde_json::Error) -> Fault<crate::lanes!(Fatal)> {
    match lane_table(&e) {
        Fault::Fatal(f) => f.with_source(e).into(),
    }
}

/// [`classify_serde_json_fault`] for a borrowed error — the same
/// [`lane_table`], with the message folded into `context` in place of the
/// source, which cannot be moved out of a shared reference.
/// [`crate::Fault::classify`] calls this after finding a `serde_json::Error`
/// in a chain it cannot otherwise classify.
pub(crate) fn classify_serde_json_ref(e: &::serde_json::Error) -> Fault<crate::lanes!(Fatal)> {
    match lane_table(e) {
        Fault::Fatal(f) => f.with_context(e.to_string()).into(),
    }
}

/// `serde_json::Error` never rejects — it decodes or encodes stored bytes,
/// never caller input (see the module docs). Enters the lanes with bare `?`
/// through the `Classify` blankets.
impl crate::Classify for ::serde_json::Error {
    type Rejected = core::convert::Infallible;
    type Lanes = crate::lanes!(Fatal);

    fn classify(self) -> Fail<Self::Rejected, Self::Lanes> {
        match classify_serde_json_fault(self) {
            Fault::Fatal(x) => Fail::Fatal(x),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lane::Lane;

    fn parse_error() -> ::serde_json::Error {
        ::serde_json::from_str::<serde_json::Value>("{ not json").unwrap_err()
    }

    #[test]
    fn syntax_error_is_fatal_invariant() {
        let f = classify_serde_json_fault(parse_error());
        match f {
            Fault::Fatal(fatal) => assert_eq!(fatal.kind, FatalKind::Invariant),
            other => panic!("expected Fatal(Invariant), got {other:?}"),
        }
    }

    #[test]
    fn classify_serde_json_widens_into_any_rejection_type() {
        #[derive(Debug)]
        struct NeverRejects;
        let f: Fail<NeverRejects> = parse_error().into();
        assert_eq!(f.lane(), Lane::Fatal);
    }

    #[test]
    fn classify_ref_folds_the_message_into_context() {
        let e = parse_error();
        let message = e.to_string();
        match classify_serde_json_ref(&e) {
            Fault::Fatal(f) => {
                assert_eq!(f.context.as_deref(), Some(message.as_str()));
            }
            other => panic!("expected Fatal, got {other:?}"),
        }
    }
}
