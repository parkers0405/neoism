use futures::{future::BoxFuture, FutureExt};
use neoism_agent_composition::*;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

#[derive(Default)]
struct Host {
    started: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    finished: Arc<AtomicUsize>,
}
impl ToolHost for Host {
    fn catalog(&self) -> Vec<ToolDescriptor> {
        ["test.echo", "test.delay", "test.fail"]
            .into_iter()
            .map(|path| ToolDescriptor {
                path: path.into(),
                description: format!("{path} test tool"),
                signature: "(input: JSON) => JSON".into(),
            })
            .collect()
    }
    fn call(
        &self,
        path: String,
        input: Value,
        cancel: Arc<AtomicBool>,
    ) -> BoxFuture<'static, Result<Value, String>> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let active = self.active.clone();
        let peak = self.peak.clone();
        let finished = self.finished.clone();
        async move {
            struct Active(Arc<AtomicUsize>);
            impl Drop for Active {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let n = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(n, Ordering::SeqCst);
            let _active = Active(active);
            if path == "test.fail" {
                return Err("test failure".into());
            }
            if path == "test.delay" {
                for _ in 0..20 {
                    if cancel.load(Ordering::SeqCst) {
                        return Err("cancelled by host".into());
                    }
                    tokio::time::sleep(Duration::from_millis(3)).await;
                }
            }
            finished.fetch_add(1, Ordering::SeqCst);
            Ok(input)
        }
        .boxed()
    }
}
async fn run(code: &str) -> ExecutionResult {
    execute(
        code,
        Arc::new(Host::default()),
        Limits::default(),
        Arc::new(AtomicBool::new(false)),
    )
    .await
}
fn value(result: ExecutionResult) -> Value {
    assert!(result.ok, "{:?}", result.error);
    result.value
}
#[tokio::test]
async fn functional_core() {
    assert_eq!(value(run("const a = [1,2,3,4]; const b = a.filter(x => x % 2 === 0).map(x => x * 3); return {b, sum: b.reduce((a,b) => a+b,0), text:b.join('-')};").await),json!({"b":[6,12],"sum":18,"text":"6-12"}));
    assert_eq!(value(run("let total=0; for(let i=0;i<5;i++){if(i===2)continue;total+=i;} for(const x of [3,4]){total+=x;} let n=0;while(n<3){n++;if(n===2)break;} return total+n;").await),json!(17));
    assert_eq!(value(run("function add(x){return y=>x+y;} const f=add(4); return [f(3), !false, 2**3, null ?? 5, true?1:2];").await),json!([7,true,8,5,1]));
}
#[tokio::test]
async fn data_builtins() {
    assert_eq!(value(run("const x=JSON.parse('{\"a\":2}'); return [Object.keys(x),Object.values(x),Object.fromEntries(Object.entries(x)),JSON.stringify(x),Math.floor(2.9),' AbC '.trim().toLowerCase(), 'ab'.repeat(2)];").await),json!([["a"],[2],{"a":2},"{\"a\":2}",2,"abc","abab"]));
    assert_eq!(value(run("return [[1,2].flatMap(x=>[x,x+1]),[1,2,3].slice(-2),[1,2].find(x=>x===2),[1,2].includes(3),[1,2].every(x=>x>0),'a,b'.split(',')];").await),json!([[1,2,2,3],[2,3],2,false,true,["a","b"]]));
    assert_eq!(
        value(run("let x={a:[1,2]}; x.a[0]=7; return x;").await),
        json!({"a":[7,2]})
    );
}
#[tokio::test]
async fn dependent_and_reusable_promises() {
    let r=run("const p=tools.test.echo({x:3});const a=await p;const b=await p; return await tools.test.echo({sum:a.x+b.x});").await;
    assert_eq!(r.tool_calls.len(), 2);
    assert!(r.tool_calls.iter().all(|r| r.status == "completed"));
    assert_eq!(value(r), json!({"sum":6}));
}
#[tokio::test]
async fn parallel_tools_and_async_callbacks() {
    let host = Arc::new(Host::default());
    let limits = Limits {
        concurrency: 2,
        ..Limits::default()
    };
    let r=execute("return await Promise.all([1,2,3,4].map(async x => { const a=await tools.test.delay({x});return a.x*2; }));",host.clone(),limits,Arc::new(AtomicBool::new(false))).await;
    assert_eq!(value(r), json!([2, 4, 6, 8]));
    assert_eq!(host.peak.load(Ordering::SeqCst), 2);
    assert_eq!(host.active.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn discovery_and_errors() {
    let r = value(
        run("return search({query:'test',namespace:'test',limit:2,offset:1});").await,
    );
    assert_eq!(r["total"], 3);
    assert_eq!(r["tools"][0]["path"], "tools.test.delay");
    assert_eq!(r["nextOffset"], Value::Null);
    assert_eq!(
        value(run("try {await tools.test.fail({});}catch(e){return e.message;}").await),
        json!("test failure")
    );
    let r=value(run("return await Promise.allSettled([tools.test.echo({x:1}),tools.test.fail({})]);").await);
    assert_eq!(r[0]["status"], "fulfilled");
    assert_eq!(r[1]["status"], "rejected");
    assert_eq!(
        value(run("try {throw 'ouch';}catch(e){return e.message;}").await),
        json!("ouch")
    );
    assert!(!run("return tools.test.unavailable({});").await.ok);
}
#[tokio::test]
async fn unsupported_program_has_no_effects() {
    for suffix in [
        "class X {}",
        "import x from 'x';",
        "return /abc/;",
        "const {x}=a;",
        "return new Date();",
        "return tools.constructor;",
    ] {
        let host = Arc::new(Host::default());
        let code = format!("await tools.test.echo({{}});{suffix}");
        let r = execute(
            &code,
            host.clone(),
            Limits::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        assert!(!r.ok, "{suffix}");
        assert_eq!(host.started.load(Ordering::SeqCst), 0, "{suffix}");
    }
    assert_eq!(run("return (;").await.error.unwrap().kind, "syntax");
}
#[tokio::test]
async fn limits_are_not_catchable() {
    let host = Arc::new(Host::default());
    let limits = Limits {
        max_steps: 250,
        ..Limits::default()
    };
    let r = execute(
        "try{while(true){}}catch(e){return 1;}",
        host,
        limits,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().kind, "limit");
    assert!(!run("return 'x'.repeat(1000000000);").await.ok);
    let r = execute(
        "return 'x'.repeat(1000);",
        Arc::new(Host::default()),
        Limits {
            max_data_bytes: 512,
            ..Limits::default()
        },
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(!r.ok);
    assert!(!run("return 1/0;").await.ok);
    assert!(!run("function f(){return f();} return f();").await.ok);
}
#[tokio::test]
async fn cancellation_timeout_and_pending_cleanup() {
    let host = Arc::new(Host::default());
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        c.store(true, Ordering::SeqCst);
    });
    let r = execute(
        "try{return await tools.test.delay({});}catch(e){return 1;}",
        host.clone(),
        Limits::default(),
        cancel,
    )
    .await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().kind, "cancelled");
    assert_eq!(host.active.load(Ordering::SeqCst), 0);
    let r = execute(
        "return await tools.test.delay({});",
        host.clone(),
        Limits {
            wall_clock: Duration::from_millis(10),
            ..Limits::default()
        },
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().kind, "timeout");
    assert_eq!(host.active.load(Ordering::SeqCst), 0);
    let r = execute(
        "tools.test.delay({});return 7;",
        host.clone(),
        Limits::default(),
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert_eq!(value(r.clone()), json!(7));
    assert_eq!(r.tool_calls[0].status, "interrupted");
    let finished = host.finished.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(host.finished.load(Ordering::SeqCst), finished);
}
#[tokio::test]
async fn security_and_isolation() {
    for code in [
        "tools = {};return 1;",
        "const tools={};return 1;",
        "const t=tools; t.test.echo=1;return 1;",
        "const t=JSON; t.parse=1;return 1;",
        "return {}['con'+'structor'];",
        "return JSON.parse('{\"__proto__\":1}');",
        "return globalThis;",
        "return process;",
        "return eval('1');",
    ] {
        assert!(!run(code).await.ok, "{code}");
    }
    assert_eq!(value(run("let x=1; x+=3; return x;").await), json!(4));
    assert!(!run("return x;").await.ok);
}
#[tokio::test]
async fn retained_output_and_log_bounds() {
    let limits = Limits {
        max_log_bytes: 16,
        max_output_bytes: 32,
        ..Limits::default()
    };
    let r = execute(
        "console.log('a'.repeat(100));return 'b'.repeat(100);",
        Arc::new(Host::default()),
        limits,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(!r.ok);
    assert!(r.truncated);
    assert!(r.logs.iter().map(String::len).sum::<usize>() <= 16);
    assert_eq!(r.value, Value::Null);
    let limits = Limits {
        max_calls: 1,
        ..Limits::default()
    };
    let r = execute(
        "try{await tools.test.echo({});await tools.test.echo({});}catch(e){return 1;}",
        Arc::new(Host::default()),
        limits,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(!r.ok);
}
