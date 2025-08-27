use es_entity::context::EventContext;
use es_entity_macros::es_event_context;
use serde_json::json;

struct TestStruct;

impl TestStruct {
    #[es_event_context(value, count)]
    async fn test_arg_capture(&self, value: &str, count: u32) -> serde_json::Value {
        EventContext::current().as_json().unwrap()
    }

    #[es_event_context]
    async fn test_no_args(&self) -> serde_json::Value {
        let mut ctx = EventContext::current();
        ctx.insert("method", &json!("no_args")).unwrap();
        ctx.as_json().unwrap()
    }
}

#[tokio::test]
async fn es_event_context_macro_integration() {
    let mut ctx = EventContext::current();
    ctx.insert("initial", &json!("data")).unwrap();
    assert_eq!(ctx.as_json().unwrap(), json!({ "initial": "data" }));

    let test_struct = TestStruct;
    let result = test_struct.test_arg_capture("hello", 42).await;
    assert_eq!(
        result,
        json!({
            "initial": "data",
            "value": "hello",
            "count": 42
        })
    );

    assert_eq!(
        EventContext::current().as_json().unwrap(),
        json!({ "initial": "data" })
    );

    let result = test_struct.test_no_args().await;
    assert_eq!(
        result,
        json!({
            "initial": "data",
            "method": "no_args"
        })
    );

    assert_eq!(
        EventContext::current().as_json().unwrap(),
        json!({ "initial": "data" })
    );
}
