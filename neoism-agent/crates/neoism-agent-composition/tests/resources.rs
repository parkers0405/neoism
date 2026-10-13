use futures::{future::BoxFuture, FutureExt};
use neoism_agent_composition::*;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

#[derive(Default)]
struct PendingHost {
    active: Arc<AtomicUsize>,
    cancellation: Arc<std::sync::Mutex<Option<Arc<AtomicBool>>>>,
}
impl ToolHost for PendingHost {
    fn catalog(&self) -> Vec<ToolDescriptor> {
        vec![ToolDescriptor {
            path: "test.wait".into(),
            description: "wait forever".into(),
            signature: "(json) => json".into(),
        }]
    }
    fn call(
        &self,
        _: String,
        _: Value,
        cancel: Arc<AtomicBool>,
    ) -> BoxFuture<'static, Result<Value, String>> {
        *self.cancellation.lock().unwrap() = Some(cancel);
        let active = self.active.clone();
        async move {
            struct Guard(Arc<AtomicUsize>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::SeqCst);
                }
            }
            active.fetch_add(1, Ordering::SeqCst);
            let _guard = Guard(active);
            futures::future::pending().await
        }
        .boxed()
    }
}
fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}
#[tokio::test]
async fn dropping_execution_aborts_owned_tasks() {
    let host = Arc::new(PendingHost::default());
    let h = host.clone();
    let task = tokio::spawn(async move {
        execute(
            "return await tools.test.wait({});",
            h,
            Limits::default(),
            cancel(),
        )
        .await
    });
    for _ in 0..100 {
        if host.active.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(host.active.load(Ordering::SeqCst), 1);
    task.abort();
    let _ = task.await;
    for _ in 0..100 {
        if host.active.load(Ordering::SeqCst) == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(host.active.load(Ordering::SeqCst), 0);
    assert!(host
        .cancellation
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .load(Ordering::SeqCst));
}
#[tokio::test]
async fn noncooperative_host_future_is_dropped() {
    let host = Arc::new(PendingHost::default());
    let r = execute(
        "return await tools.test.wait({});",
        host.clone(),
        Limits {
            wall_clock: Duration::from_millis(15),
            cleanup_timeout: Duration::from_millis(15),
            ..Limits::default()
        },
        cancel(),
    )
    .await;
    assert!(!r.ok);
    assert_eq!(host.active.load(Ordering::SeqCst), 0);
    assert_eq!(r.tool_calls[0].status, "interrupted");
}
#[tokio::test]
async fn synchronous_spin_yields_for_cancellation() {
    let flag = cancel();
    let f = flag.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1)).await;
        f.store(true, Ordering::SeqCst);
    });
    let r = execute(
        "try {while(true){}} catch(e){return 1;}",
        Arc::new(PendingHost::default()),
        Limits {
            max_steps: 10_000_000,
            ..Limits::default()
        },
        flag,
    )
    .await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().kind, "cancelled");
}
#[tokio::test]
async fn parser_and_runtime_depth_are_bounded() {
    let host = Arc::new(PendingHost::default());
    for code in [
        format!("return {}0{};", "(".repeat(300), ")".repeat(300)),
        format!("return {}0;", "!\n".repeat(10000)),
        format!("{}return 0;", "if(true);else ".repeat(400)),
    ] {
        let r = execute(&code, host.clone(), Limits::default(), cancel()).await;
        assert!(!r.ok);
        assert_eq!(r.error.unwrap().kind, "limit");
    }
    let r = execute(
        "let x=[];for(let i=0;i<100;i++){x=[x];}return 1;",
        host.clone(),
        Limits::default(),
        cancel(),
    )
    .await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().kind, "limit");
    let r = execute(
        "return 1;",
        host,
        Limits {
            max_source_bytes: 3,
            ..Limits::default()
        },
        cancel(),
    )
    .await;
    assert!(!r.ok);
}
#[tokio::test]
async fn utf8_and_source_locations() {
    let host = Arc::new(PendingHost::default());
    let r = execute(
        "\n\nreturn nonexistent;",
        host.clone(),
        Limits::default(),
        cancel(),
    )
    .await;
    assert_eq!(r.error.unwrap().line, Some(3));
    let r = execute("\nreturn (;", host.clone(), Limits::default(), cancel()).await;
    assert_eq!(r.error.unwrap().line, Some(2));
    let r = execute(
        "console.log('🎉'.repeat(50));return '🎉'.slice(0,1);",
        host,
        Limits {
            max_log_bytes: 7,
            ..Limits::default()
        },
        cancel(),
    )
    .await;
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.value, json!("🎉"));
    assert!(r.truncated);
    assert_eq!(r.logs[0], "🎉");
}
#[tokio::test]
async fn call_record_input_retention_is_bounded() {
    let r = execute(
        "tools.test.wait({data:'a'.repeat(1000)});return 1;",
        Arc::new(PendingHost::default()),
        Limits {
            max_output_bytes: 32,
            ..Limits::default()
        },
        cancel(),
    )
    .await;
    assert!(r.ok, "{:?}", r.error);
    assert!(r.truncated);
    assert_eq!(r.tool_calls[0].input, Value::Null);
    assert_eq!(r.tool_calls[0].status, "interrupted");
}
#[tokio::test]
async fn expression_target_and_coercive_equality_rejected_before_effects() {
    for code in [
        "tools.test.wait({}); let a={}; a['x'+'y']=1;return 1;",
        "tools.test.wait({});return 1=='1';",
    ] {
        let host = Arc::new(PendingHost::default());
        let r = execute(code, host.clone(), Limits::default(), cancel()).await;
        assert!(!r.ok);
        assert!(r.tool_calls.is_empty());
        assert!(host.cancellation.lock().unwrap().is_none());
    }
}
