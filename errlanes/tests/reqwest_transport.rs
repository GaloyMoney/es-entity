#![cfg(feature = "classify-reqwest")]

use std::{
    error::Error,
    io::{ErrorKind, Read},
    net::TcpListener,
    time::Duration,
};

use errlanes::{Classify, Fail, FatalKind, Fault, TransientKind};

fn has_connection_reset(error: &(dyn Error + 'static)) -> bool {
    let mut source = Some(error);
    while let Some(error) = source {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == ErrorKind::ConnectionReset)
        {
            return true;
        }
        source = error.source();
    }
    false
}

#[tokio::test]
async fn reset_after_connect_is_transient_and_preserves_the_source() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
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
        // Abort an established connection after receiving the request.
        socket2::SockRef::from(&stream)
            .set_linger(Some(Duration::ZERO))
            .unwrap();
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
    assert!(has_connection_reset(&error), "{error:?}");

    let borrowed = Fault::classify(&error);
    assert!(
        matches!(borrowed, Fault::Transient(ref t) if t.kind == TransientKind::ConnectionLost),
        "{borrowed:?}"
    );
    let owned = error.classify();
    let Fail::Transient(transient) = owned else {
        panic!("expected connection lost, got {owned:?}");
    };
    assert_eq!(transient.kind, TransientKind::ConnectionLost);
    assert!(has_connection_reset(&transient));
    assert!(
        transient
            .source()
            .unwrap()
            .downcast_ref::<reqwest::Error>()
            .is_some()
    );
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
        error.classify(),
        Fail::Fatal(fatal) if fatal.kind == FatalKind::Invariant
    ));
}
