use es_entity::context::EventContext;
use es_entity_macros::es_event_context;
use serde_json::json;

struct TestStruct;

impl TestStruct {
    #[es_event_context(value, count)]
    async fn test_method(&self, value: &str, count: u32) -> serde_json::Value {
        // Inside the macro-wrapped method, we should have a new context
        // with the arguments inserted
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
    // Set up initial context
    let mut ctx = EventContext::current();
    ctx.insert("initial", &json!("data")).unwrap();
    assert_eq!(ctx.as_json().unwrap(), json!({ "initial": "data" }));

    // Test with arguments
    let test_struct = TestStruct;
    let result = test_struct.test_method("hello", 42).await;
    assert_eq!(
        result,
        json!({
            "initial": "data",
            "value": "hello",
            "count": 42
        })
    );

    // After the method call, the context will have the inserted arguments
    // because it's the same thread-local context
    assert_eq!(
        EventContext::current().as_json().unwrap(),
        json!({ "initial": "data", "value": "hello", "count": 42 })
    );

    // Test without arguments - it will inherit the current context
    let result = test_struct.test_no_args().await;
    assert_eq!(
        result,
        json!({
            "initial": "data",
            "value": "hello",
            "count": 42,
            "method": "no_args"
        })
    );

    // The outer context still has the accumulated values from the first method call
    // but not the "method" key since that was added in a nested context
    assert_eq!(
        EventContext::current().as_json().unwrap(),
        json!({ "initial": "data", "value": "hello", "count": 42 })
    );
}

#[tokio::test]
async fn es_event_context_macro_spawned() {
    use tokio::spawn;

    struct Service;

    impl Service {
        #[es_event_context(request_id)]
        async fn handle_request(&self, request_id: u64) -> String {
            let ctx = EventContext::current();
            format!("Handled request with context: {:?}", ctx.as_json().unwrap())
        }
    }

    // Set initial context
    let mut ctx = EventContext::current();
    ctx.insert("service", &json!("main")).unwrap();

    let service = Service;

    // Spawn a task with the macro-wrapped method
    let handle = spawn(async move { service.handle_request(123).await });

    let result = handle.await.unwrap();
    assert!(result.contains("request_id"));
    assert!(result.contains("123"));
    assert!(result.contains("service"));
    assert!(result.contains("main"));
}
