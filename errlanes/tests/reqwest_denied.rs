#![cfg(feature = "classify-reqwest")]
use std::error::Error;

use errlanes::{Classify, Fail, FatalKind, Fault, Lane, TransientKind, lanes};

type Lanes = lanes!(Denied, Transient, Fatal);

fn status_error(code: u16) -> reqwest::Error {
    reqwest::Response::from(http::Response::builder().status(code).body("").unwrap())
        .error_for_status()
        .unwrap_err()
}

fn reqwest_in<'a>(e: &'a (dyn Error + 'static)) -> Option<&'a reqwest::Error> {
    let mut cur = Some(e);
    while let Some(x) = cur {
        if let Some(r) = x.downcast_ref::<reqwest::Error>() {
            return Some(r);
        }
        cur = x.source();
    }
    None
}

fn owned(e: reqwest::Error) -> Fault<Lanes> {
    match e.classify() {
        Fail::Denied(d) => Fault::Denied(d),
        Fail::Transient(t) => Fault::Transient(t),
        Fail::Fatal(f) => Fault::Fatal(f),
        Fail::Rejected(never) => match never {},
    }
}

fn assert_retains_native(e: &(dyn Error + 'static), code: u16) {
    let native = reqwest_in(e).expect("the native reqwest::Error is in the source chain");
    assert_eq!(native.status().map(|s| s.as_u16()), Some(code));
    assert!(
        native.url().is_some(),
        "the URL is still on the native error"
    );
}

#[test]
fn owned_denied_retains_the_native_error() {
    for code in [401, 403] {
        let Fault::Denied(d) = owned(status_error(code)) else {
            panic!("{code} should be denied");
        };
        assert_retains_native(&d, code);
    }
}

#[test]
fn narrowed_owned_denied_keeps_the_chain_and_message() {
    for code in [401, 403] {
        let url = status_error(code).url().unwrap().to_string();
        let narrowed = owned(status_error(code)).narrow_denied();
        let Fault::Fatal(f) = &narrowed else {
            panic!("narrowed denied is fatal");
        };
        assert_eq!(f.kind, FatalKind::Denied);
        assert_retains_native(f, code);
        let message = narrowed.message();
        assert!(message.contains(&code.to_string()), "{message}");
        assert!(message.contains(&url), "{message}");

        let cloned = narrowed.clone();
        assert_retains_native(cloned.as_fatal().unwrap(), code);
        let widened: Fault = cloned.widen();
        assert_retains_native(widened.as_fatal().unwrap(), code);
    }
}

#[test]
fn erased_boundary_recovers_the_denied_with_its_native_error() {
    let boxed: Box<dyn Error + Send + Sync> = Box::new(owned(status_error(403)));
    let recovered = Fault::classify(boxed.as_ref());
    let Fault::Denied(d) = &recovered else {
        panic!("denied survives the box");
    };
    assert_retains_native(d, 403);
    let narrowed = recovered.narrow_denied();
    assert_retains_native(narrowed.as_fatal().unwrap(), 403);
    assert!(narrowed.message().contains("403"));
}

#[test]
fn borrowed_denied_keeps_status_and_url_as_context_through_narrowing() {
    for code in [401, 403] {
        let e = status_error(code);
        let url = e.url().unwrap().to_string();
        let fault = Fault::classify(&e);
        assert_eq!(fault.lane(), Lane::Denied);
        let narrowed = fault.narrow_denied();
        let message = narrowed.message();
        assert!(message.contains(&code.to_string()), "{message}");
        assert!(message.contains(&url), "{message}");
        assert_eq!(
            message.matches(&url).count(),
            1,
            "context must not be duplicated: {message}"
        );
    }
}

#[test]
fn other_statuses_keep_their_lane_and_source() {
    for (code, lane) in [
        (400, Lane::Fatal),
        (429, Lane::Transient),
        (503, Lane::Transient),
    ] {
        let fault = owned(status_error(code));
        assert_eq!(fault.lane(), lane, "{code}");
        let source = match &fault {
            Fault::Transient(t) => t.source(),
            Fault::Fatal(f) => f.source(),
            Fault::Denied(_) => panic!("{code} is not denied"),
        };
        assert_retains_native(source.expect("source retained"), code);
    }
    assert!(matches!(
        owned(status_error(429)),
        Fault::Transient(t) if t.kind == TransientKind::Congestion
    ));
}

#[tokio::test]
async fn transport_failure_keeps_its_lane_and_source() {
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let e = reqwest::Client::new()
        .get(format!("http://{addr}/"))
        .send()
        .await
        .unwrap_err();
    assert!(e.is_connect());
    let Fault::Transient(t) = owned(e) else {
        panic!("connect failure is transient");
    };
    assert_eq!(t.kind, TransientKind::ConnectionLost);
    assert!(reqwest_in(&t).is_some());
}
