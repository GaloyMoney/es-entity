#![cfg(feature = "classify-reqwest")]
use std::{
    error::Error,
    io::{self, Read},
    net::TcpListener,
    time::Duration,
};

use errlanes::{Classify, Fail, FatalKind, Fault, Lane, TransientKind, lanes};

type Lanes = lanes!(Denied, Transient, Fatal);

fn status_error(code: u16) -> reqwest::Error {
    reqwest::Response::from(http::Response::builder().status(code).body("").unwrap())
        .error_for_status()
        .unwrap_err()
}

fn source_in<'a, T: Error + 'static>(e: &'a (dyn Error + 'static)) -> Option<&'a T> {
    let mut cur = Some(e);
    while let Some(x) = cur {
        if let Some(r) = x.downcast_ref::<T>() {
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
    let native =
        source_in::<reqwest::Error>(e).expect("the native reqwest::Error is in the source chain");
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
        let converted: Fault = cloned.into();
        assert_retains_native(converted.as_fatal().unwrap(), code);
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
    assert!(source_in::<reqwest::Error>(&t).is_some());
}

#[tokio::test]
async fn reset_after_connect_is_transient_and_preserves_the_source() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        // A client timeout cannot interrupt a blocking accept in this thread.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "client did not connect"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 1);
        socket2::SockRef::from(&stream)
            .set_linger(Some(Duration::ZERO))
            .unwrap();
        // Abort the established connection after receiving the request.
        drop(stream);
    });
    let error = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .retry(reqwest::retry::never())
        .build()
        .unwrap()
        .get(format!("http://{address}/"))
        .send()
        .await
        .unwrap_err();
    server.join().unwrap();

    assert!(error.is_request());
    assert!(
        !error.is_connect(),
        "reset occurred after connection establishment"
    );
    assert!(!error.is_timeout());
    assert_eq!(
        source_in::<io::Error>(&error).unwrap().kind(),
        io::ErrorKind::ConnectionReset
    );

    let borrowed = Fault::classify(&error);
    assert!(
        matches!(borrowed, Fault::Transient(ref t) if t.kind == TransientKind::ConnectionLost),
        "{borrowed:?}"
    );
    let fault = owned(error);
    let Fault::Transient(transient) = fault else {
        panic!("expected connection lost, got {fault:?}");
    };
    assert_eq!(transient.kind, TransientKind::ConnectionLost);
    assert_eq!(
        source_in::<io::Error>(&transient).unwrap().kind(),
        io::ErrorKind::ConnectionReset
    );
    assert!(source_in::<reqwest::Error>(&transient).is_some());
}

#[tokio::test]
async fn malformed_request_remains_fatal() {
    let error = reqwest::Client::new()
        .get("http://127.0.0.1/")
        .header("invalid header name", "value")
        .send()
        .await
        .unwrap_err();
    assert!(error.is_builder());
    assert!(matches!(
        owned(error),
        Fault::Fatal(fatal) if fatal.kind == FatalKind::Invariant
    ));
}
