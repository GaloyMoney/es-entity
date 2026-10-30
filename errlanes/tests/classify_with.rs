//! `#[classify(with = ..)]` structs bind their fields in `#[error(..)]` and
//! emit `source()` exactly as laned structs do; the enum `with` path keeps
//! its unit `Display` and no `source()`.

use std::{convert::Infallible, error::Error};

use errlanes::{Fail, Fault, lanes};

type Tf = lanes!(Transient, Fatal);

fn fail_fatal<E: Error + Send + Sync + 'static>(e: E) -> Fail<Infallible, Tf> {
    Fail::Fatal(errlanes::Fatal::from_error(
        errlanes::FatalKind::Dependency,
        e,
    ))
}

fn classify_upstream(u: Upstream) -> Fail<Infallible, Tf> {
    if u.errors.iter().any(|e| e == "EGeneral:No data") {
        Fail::Transient(errlanes::Transient::from_error(
            errlanes::TransientKind::UpstreamUnavailable,
            u,
        ))
    } else {
        fail_fatal(u)
    }
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_upstream, lanes(Transient, Fatal))]
#[error("Kraken rejected request: {errors:?}")]
struct Upstream {
    errors: Vec<String>,
}

#[test]
fn kraken_shape_interpolates_named_field_and_classifies_whole_struct() {
    let u = Upstream {
        errors: vec!["EGeneral:No data".to_string()],
    };
    assert_eq!(
        u.to_string(),
        "Kraken rejected request: [\"EGeneral:No data\"]"
    );
    let fail: Fail<Infallible, Tf> = errlanes::Classify::classify(u);
    let fault: Fault<Tf> = fail.into_fault();
    assert!(fault.is_transient());
    assert!(
        fault
            .message()
            .contains("Kraken rejected request: [\"EGeneral:No data\"]")
    );

    let fatal = Upstream {
        errors: vec!["EOrder:Insufficient funds".to_string()],
    };
    let fault: Fault<Tf> = errlanes::Classify::classify(fatal).into_fault();
    assert!(fault.is_fatal());
}

fn classify_pair(p: Pair) -> Fail<Infallible, Tf> {
    fail_fatal(p)
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_pair, lanes(Transient, Fatal))]
#[error("pair {0} / {1:?}")]
struct Pair(u32, String);

#[test]
fn tuple_struct_positional_placeholders_interpolate() {
    assert_eq!(Pair(7, "x".into()).to_string(), "pair 7 / \"x\"");
}

fn classify_marked(m: Marked) -> Fail<Infallible, Tf> {
    fail_fatal(m)
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_marked, lanes(Transient, Fatal))]
#[error("marked: {context}")]
struct Marked {
    #[source]
    cause: std::io::Error,
    context: String,
}

fn classify_named_source(m: NamedSource) -> Fail<Infallible, Tf> {
    fail_fatal(m)
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_named_source, lanes(Transient, Fatal))]
#[error("named source")]
struct NamedSource {
    source: std::io::Error,
}

#[test]
fn source_attribute_and_source_named_field_emit_source() {
    let m = Marked {
        cause: std::io::Error::other("disk"),
        context: "ctx".into(),
    };
    assert_eq!(m.to_string(), "marked: ctx");
    let src = m.source().expect("source is Some");
    assert_eq!(
        src.downcast_ref::<std::io::Error>().unwrap().to_string(),
        "disk"
    );

    let n = NamedSource {
        source: std::io::Error::other("net"),
    };
    let src = n.source().expect("source is Some");
    assert_eq!(
        src.downcast_ref::<std::io::Error>().unwrap().to_string(),
        "net"
    );
}

fn classify_from(t: FromPayload) -> Fail<Infallible, Tf> {
    fail_fatal(t)
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_from, lanes(Transient, Fatal), from)]
#[error("from payload")]
struct FromPayload(std::io::Error);

#[test]
fn from_with_single_payload_emits_from_and_source() {
    let t: FromPayload = std::io::Error::other("boom").into();
    assert_eq!(t.to_string(), "from payload");
    let src = t.source().expect("source is Some");
    assert_eq!(
        src.downcast_ref::<std::io::Error>().unwrap().to_string(),
        "boom"
    );
}

fn classify_generic<T: std::fmt::Debug + Send + Sync + 'static>(w: W<T>) -> Fail<Infallible, Tf> {
    fail_fatal(w)
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_generic, lanes(Transient, Fatal))]
#[error("inner {inner:?}")]
struct W<T> {
    inner: T,
}

#[test]
fn generic_with_struct_bounds_land_on_the_impl() {
    assert_eq!(W { inner: 3u8 }.to_string(), "inner 3");
}

fn classify_literal(l: Literal) -> Fail<Infallible, Tf> {
    fail_fatal(l)
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_literal, lanes(Transient, Fatal))]
#[error("a bare literal")]
struct Literal;

fn classify_unnamed(l: NoErrorAttr) -> Fail<Infallible, Tf> {
    fail_fatal(l)
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_unnamed, lanes(Transient, Fatal))]
struct NoErrorAttr {
    _detail: u8,
}

#[test]
fn bare_literal_and_missing_error_attr_render_as_before() {
    assert_eq!(Literal.to_string(), "a bare literal");
    assert!(Literal.source().is_none());
    assert_eq!(NoErrorAttr { _detail: 1 }.to_string(), "no_error_attr");
    assert!(NoErrorAttr { _detail: 1 }.source().is_none());
}

fn classify_manual(m: Manual) -> Fail<Infallible, Tf> {
    Fail::Fatal(errlanes::Fatal::new(errlanes::FatalKind::Dependency).with_context(m.0))
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_manual, lanes(Transient, Fatal), error = manual)]
struct Manual(&'static str);

impl std::fmt::Display for Manual {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "hand-written {}", self.0)
    }
}
impl Error for Manual {}

#[test]
fn error_manual_with_struct_emits_no_display_or_error() {
    assert_eq!(Manual("m").to_string(), "hand-written m");
}

fn classify_enum(e: EnumWith) -> Fail<Infallible, Tf> {
    fail_fatal(e)
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_enum, lanes(Transient, Fatal))]
#[allow(dead_code)]
enum EnumWith {
    Io(#[source] std::io::Error),
    Plain { _n: u8 },
}

fn classify_enum_literal(e: EnumLiteral) -> Fail<Infallible, Tf> {
    fail_fatal(e)
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_enum_literal, lanes(Transient, Fatal))]
#[error("enum level literal")]
#[allow(dead_code)]
enum EnumLiteral {
    Io(std::io::Error),
}

#[test]
fn enum_with_keeps_unit_display_and_no_source() {
    let io = EnumWith::Io(std::io::Error::other("x"));
    assert_eq!(io.to_string(), "enum_with");
    assert!(io.source().is_none());
    let plain = EnumWith::Plain { _n: 1 };
    assert_eq!(plain.to_string(), "enum_with");
    assert!(plain.source().is_none());

    let lit = EnumLiteral::Io(std::io::Error::other("y"));
    assert_eq!(lit.to_string(), "enum level literal");
    assert!(lit.source().is_none());
}
