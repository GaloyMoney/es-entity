use errlanes::{Fail, FatalKind, Lift, Rejection, ResultExt, Transient, TransientKind, lanes};
use std::{error::Error, time::Duration};

#[derive(Debug, errlanes::Rejection)]
enum SubscriptionRejection {
    CaughtUpTimeout {
        checkpoint: u64,
        target: u64,
        waited: Duration,
    },
    NoSuchJob {
        key: String,
    },
}

#[derive(Debug, PartialEq, errlanes::Rejection, errlanes::Lift)]
#[rejection(code = "EC_CAUGHT_UP_TIMEOUT")]
#[lift(SubscriptionRejection, variant = CaughtUpTimeout, unhandled = fatal)]
struct EcCaughtUpTimeout {
    #[lift(from = checkpoint)]
    applied: u64,
    #[lift(from = target)]
    frontier: u64,
    waited: Duration,
}

#[test]
fn selected_variant_moves_renamed_fields_and_keeps_destination_metadata() {
    let waited = Duration::from_secs(2);
    let result: Result<(), Fail<EcCaughtUpTimeout, lanes!(Fatal)>> =
        Err(SubscriptionRejection::CaughtUpTimeout {
            checkpoint: 3,
            target: 8,
            waited,
        })
        .lift();
    let Fail::Rejected(timeout) = result.unwrap_err() else {
        panic!("timeout rejection")
    };
    assert_eq!(
        timeout,
        EcCaughtUpTimeout {
            applied: 3,
            frontier: 8,
            waited
        }
    );
    assert_eq!(
        Into::<&'static str>::into(timeout.code()),
        "EC_CAUGHT_UP_TIMEOUT"
    );
}

#[test]
fn unhandled_variant_is_preserved_and_expands_to_an_invariant_fault() {
    let source = EcCaughtUpTimeout::lift(SubscriptionRejection::NoSuchJob {
        key: "missing".into(),
    })
    .unwrap_err();
    assert!(matches!(&source, SubscriptionRejection::NoSuchJob { key } if key == "missing"));
    let result: Result<(), Fail<EcCaughtUpTimeout, lanes!(Fatal)>> = Err(source).lift();
    let Fail::Fatal(fatal) = result.unwrap_err() else {
        panic!("invariant fault")
    };
    assert_eq!(fatal.kind, FatalKind::Invariant);
    assert!(matches!(
        fatal.source().unwrap().downcast_ref::<SubscriptionRejection>(),
        Some(SubscriptionRejection::NoSuchJob { key }) if key == "missing"
    ));
}

#[test]
fn lane_expansion_carriers_preserves_success_and_fault_payloads() {
    type Inner = Fail<SubscriptionRejection, lanes!(Transient, Fatal)>;
    type Outer = Fail<EcCaughtUpTimeout, lanes!(Transient, Fatal)>;
    let success: Result<_, Outer> = Ok::<_, Inner>(42).lift();
    assert_eq!(success.unwrap(), 42);
    let transient =
        Transient::new(TransientKind::Deadlock).with_source(std::io::Error::other("retry"));
    let source = transient.source_arc().unwrap().clone();
    let result: Result<(), Outer> = Err::<(), Inner>(transient.into()).lift();
    let Fail::Transient(transient) = result.unwrap_err() else {
        panic!("transient")
    };
    assert!(std::sync::Arc::ptr_eq(
        &source,
        transient.source_arc().unwrap()
    ));
}

#[derive(Debug, errlanes::Rejection)]
enum Only {
    Value { payload: String },
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[rejection(code = "TOTAL")]
#[lift(Only, variant = Value)]
struct Total {
    #[lift(from = payload)]
    value: String,
}

#[test]
fn strict_struct_lifts_generate_from_and_need_no_fatal_lane() {
    let value = Total::from(Only::Value {
        payload: "moved".into(),
    });
    assert_eq!(value.value, "moved");
    let result: Result<(), Fail<Total, lanes!()>> = Err(Only::Value {
        payload: "rejected".into(),
    })
    .map_err(Into::into);
    let Fail::Rejected(value) = result.unwrap_err();
    assert_eq!(value.value, "rejected");
}

mod generic_source {
    pub enum Source<'a, T> {
        Value { item: T, label: &'a str },
    }
}

#[derive(errlanes::Lift)]
#[lift(generic_source::Source<'a, T>, strict, variant = Value,)]
struct Generic<'a, T>
where
    T: Eq,
{
    #[lift(from = item,)]
    value: T,
    label: &'a str,
}

#[test]
fn strict_mappings_preserve_generics_lifetimes_and_where_clauses() {
    let label = String::from("borrowed");
    let result = Generic::from(generic_source::Source::Value {
        item: String::from("owned"),
        label: &label,
    });
    assert_eq!(result.value, "owned");
    assert_eq!(result.label, "borrowed");
}

enum Overlap {
    Value {
        left: String,
        right: String,
        r#type: String,
    },
}

#[derive(errlanes::Lift)]
#[lift(Overlap, variant = Value)]
struct Swapped {
    #[lift(from = right)]
    left: String,
    #[lift(from = left)]
    right: String,
    #[lift(from = r#type)]
    kind: String,
}

#[test]
fn renames_do_not_capture_other_source_fields() {
    let result = Swapped::from(Overlap::Value {
        left: "left".into(),
        right: "right".into(),
        r#type: "kind".into(),
    });
    assert_eq!(result.left, "right");
    assert_eq!(result.right, "left");
    assert_eq!(result.kind, "kind");
}
