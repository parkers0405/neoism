use futures::{future::BoxFuture, FutureExt};
use neoism_agent_composition::*;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct Host {
    calls: AtomicUsize,
    inputs: Mutex<Vec<Value>>,
    finished: Arc<AtomicBool>,
    cancelled_on_drop: Arc<AtomicBool>,
}
impl ToolHost for Host {
    fn catalog(&self) -> Vec<ToolDescriptor> {
        [
            "test.echo",
            "test.fail",
            "test.slow",
            "test.unsafe",
            "test.delete",
        ]
        .into_iter()
        .map(|path| ToolDescriptor {
            path: path.into(),
            description: String::new(),
            signature: "(json) => json".into(),
        })
        .collect()
    }
    fn call(
        &self,
        path: String,
        input: Value,
        cancel: Arc<AtomicBool>,
    ) -> BoxFuture<'static, Result<Value, String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inputs.lock().unwrap().push(input.clone());
        let finished = self.finished.clone();
        let observed = self.cancelled_on_drop.clone();
        async move {
            if path == "test.unsafe" {
                return Ok(json!({"id":9007199254740993u64}));
            }
            if path == "test.fail" {
                tokio::time::sleep(Duration::from_millis(3)).await;
                return Err("fast rejection".into());
            }
            if path == "test.slow" {
                struct Guard {
                    cancel: Arc<AtomicBool>,
                    observed: Arc<AtomicBool>,
                }
                impl Drop for Guard {
                    fn drop(&mut self) {
                        self.observed
                            .store(self.cancel.load(Ordering::SeqCst), Ordering::SeqCst);
                    }
                }
                let _guard = Guard {
                    cancel: cancel.clone(),
                    observed,
                };
                for _ in 0..100 {
                    if cancel.load(Ordering::SeqCst) {
                        return Err("cancelled".into());
                    }
                    tokio::time::sleep(Duration::from_millis(3)).await;
                }
                finished.store(true, Ordering::SeqCst);
            }
            Ok(input)
        }
        .boxed()
    }
}
fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}
async fn run(code: &str) -> ExecutionResult {
    execute(code, Arc::new(Host::default()), Limits::default(), cancel()).await
}
fn value(result: ExecutionResult) -> Value {
    assert!(result.ok, "{:?}", result.error);
    result.value
}

#[tokio::test]
async fn const_aliases_arrays_objects_and_closures_have_identity() {
    assert_eq!(
        value(run("const x={a:1};x.a=2;return x.a;").await),
        json!(2)
    );
    assert_eq!(
        value(run("let x={a:1};const y=x;x.a=2;return [y.a,x===y];").await),
        json!([2, true])
    );
    assert_eq!(value(run("const x=[1,2];const y=x;function read(){return y[0];}x[0]=7;return [read(),x===y];").await),json!([7,true]));
    assert_eq!(value(run("const x={nested:{a:1}};const y=x.nested;const f=()=>y.a;x.nested.a=9;return f();").await),json!(9));
    assert!(!run("const x={a:1};x={};return x;").await.ok);
}
#[tokio::test]
async fn cycles_are_rejected_before_insertion_and_runs_stay_isolated() {
    for _ in 0..20 {
        assert_eq!(value(run("const x={a:1};try{x.self=x;}catch(e){return [e.kind,x.a,Object.keys(x)];}return 0;").await),json!(["security",1,["a"]]));
        assert_eq!(value(run("const a={};const b={};a.b=b;try{b.a=a;}catch(e){return [e.kind,Object.keys(b)];}return 0;").await),json!(["security",[]]));
        assert_eq!(
            value(
                run("const x=[null];try{x[0]=x;}catch(e){return [e.kind,x];}return 0;")
                    .await
            ),
            json!(["security", [null]])
        );
    }
    assert!(!run("return x;").await.ok);
}
#[tokio::test]
async fn for_let_gets_fresh_body_and_update_scopes() {
    assert_eq!(value(run("let fs=[];for(let i=0;i<3;i++){fs=fs.concat(()=>i);}return fs.map(f=>f());").await),json!([0,1,2]));
    assert_eq!(value(run("let fs=[];for(let i=0;i<3;i++){fs=fs.concat(async()=>{await tools.test.echo({});return i;});}return await Promise.all(fs.map(f=>f()));").await),json!([0,1,2]));
    assert_eq!(value(run("let fs=[];for(let i=0;i<4;i++){if(i===1)continue;fs=fs.concat(()=>i);}return fs.map(f=>f());").await),json!([0,2,3]));
    assert_eq!(value(run("let updates=[];function advance(f){updates=updates.concat(f);return f()+1;}for(let i=0;i<3;i=advance(()=>i)){ }return updates.map(f=>f());").await),json!([1,2,3]));
}
#[tokio::test]
async fn prototype_literal_reads_and_mutations_reject_before_effects() {
    for suffix in [
        "return {}['constructor'];",
        "return {}['prototype'];",
        "return {}['__proto__'];",
        "const x={};x.constructor=1;",
        "const x={};x['prototype']=1;",
        "const x=[];x['__proto__']=1;",
    ] {
        let host = Arc::new(Host::default());
        let code = format!("await tools.test.echo({{}});{suffix}");
        let result = execute(&code, host.clone(), Limits::default(), cancel()).await;
        assert!(!result.ok, "{suffix}");
        assert!(result.tool_calls.is_empty());
        assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    }
    assert!(!run("const key='con'+'structor';return {}[key];").await.ok);
}
#[tokio::test]
async fn json_tool_integer_contract_and_unsafe_number_policy() {
    let host = Arc::new(Host::default());
    let result=execute("return await tools.test.echo({limit:5,line:1+2,length:[1,2,3].length,fraction:1.25});",host.clone(),Limits::default(),cancel()).await;
    assert!(result.ok, "{:?}", result.error);
    let input = host.inputs.lock().unwrap()[0].clone();
    assert_eq!(input["limit"].as_u64(), Some(5));
    assert_eq!(input["line"].as_u64(), Some(3));
    assert_eq!(input["length"].as_u64(), Some(3));
    assert_eq!(input["fraction"].as_f64(), Some(1.25));
    let typed: std::collections::BTreeMap<String, u64> =
        serde_json::from_value(json!({"limit":input["limit"],"line":input["line"]}))
            .unwrap();
    assert_eq!(typed["line"], 3);
    for code in [
        "return await tools.test.echo({id:9007199254740993});",
        "return await tools.test.echo({id:9007199254740991+2});",
        "return JSON.parse('{\"id\":9007199254740993}');",
        "return await tools.test.unsafe({});",
    ] {
        let r = run(code).await;
        assert!(!r.ok, "{code}");
        assert!(
            r.error.unwrap().message.to_lowercase().contains("unsafe"),
            "{code}"
        );
    }
    assert_eq!(value(run("return await tools.test.echo({id:'9007199254740993',safe:9007199254740991,negative:-9007199254740991});").await),json!({"id":"9007199254740993","safe":9007199254740991u64,"negative":-9007199254740991i64}));
}
#[tokio::test]
async fn undefined_policy_is_explicit_at_each_boundary() {
    let host = Arc::new(Host::default());
    for code in [
        "return await tools.test.echo({optional:undefined});",
        "return await tools.test.echo([undefined]);",
        "return await tools.test.echo(undefined);",
    ] {
        let r = execute(code, host.clone(), Limits::default(), cancel()).await;
        assert!(!r.ok);
        assert!(r.error.unwrap().message.contains("Undefined"));
        assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    }
    assert_eq!(value(run("return;").await), Value::Null);
    assert_eq!(value(run("const x=1;").await), Value::Null);
    assert_eq!(
        value(run("return JSON.stringify({x:undefined,a:[undefined,1],n:null});").await),
        json!("{\"a\":[null,1],\"n\":null}")
    );
    assert_eq!(
        value(run("return JSON.stringify(undefined);").await),
        Value::Null
    );
    assert!(!run("return {x:undefined};").await.ok);
}
#[tokio::test]
async fn promise_all_is_fail_fast_and_siblings_remain_owned() {
    let host = Arc::new(Host::default());
    let start = Instant::now();
    let r = execute(
        "return await Promise.all([tools.test.slow({}),tools.test.fail({})]);",
        host.clone(),
        Limits::default(),
        cancel(),
    )
    .await;
    assert!(!r.ok);
    assert!(start.elapsed() < Duration::from_millis(250));
    assert!(!host.finished.load(Ordering::SeqCst));
    assert!(host.cancelled_on_drop.load(Ordering::SeqCst));
    assert!(r.tool_calls.iter().any(|r| r.status == "interrupted"));
    let host = Arc::new(Host::default());
    let r=execute("const p=tools.test.slow({a:3});try{await Promise.all([p,tools.test.fail({})]);}catch(e){console.log(e.message);}return await p;",host.clone(),Limits::default(),cancel()).await;
    assert_eq!(value(r), json!({"a":3}));
    assert!(host.finished.load(Ordering::SeqCst));
    assert!(!host.cancelled_on_drop.load(Ordering::SeqCst));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_logging_rechecks_budget_atomically() {
    let list = (0..64).map(|n| n.to_string()).collect::<Vec<_>>().join(",");
    let code=format!("await Promise.all([{list}].map(async x=>{{console.log('🎉'.repeat(750));return x;}}));return 1;");
    for _ in 0..10 {
        let result = execute(
            &code,
            Arc::new(Host::default()),
            Limits {
                max_log_bytes: 16001,
                ..Limits::default()
            },
            cancel(),
        )
        .await;
        assert!(result.ok, "{:?}", result.error);
        assert!(result.logs.iter().map(String::len).sum::<usize>() <= 16001);
        assert!(result.truncated);
    }
}
#[tokio::test]
async fn json_preflight_counts_nodes_keys_and_decoded_strings() {
    let encoded = format!(
        "[{}]",
        std::iter::repeat_n("0", 1000).collect::<Vec<_>>().join(",")
    );
    let code =
        format!("const data=JSON.parse('{encoded}');return await tools.test.echo(data);");
    let host = Arc::new(Host::default());
    let result = execute(
        &code,
        host.clone(),
        Limits {
            max_data_bytes: code.len() * 64 + 30_000,
            ..Limits::default()
        },
        cancel(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().kind, "limit");
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    let key = "\\\\u0061".repeat(2000);
    let code = format!("return JSON.parse('{{\"{key}\":0}}');");
    let result = execute(
        &code,
        host,
        Limits {
            max_data_bytes: code.len() * 64 + 30_000,
            ..Limits::default()
        },
        cancel(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().kind, "limit");
    assert_eq!(
        value(run("return JSON.parse('{\"x\":[0,1,2],\"s\":\"\\\\u0061\"}');").await),
        json!({"x":[0,1,2],"s":"a"})
    );
}
#[tokio::test]
async fn zero_log_budget_and_long_key_mutation_and_trim() {
    let r = execute(
        "const s='x'.repeat(40000);console.log(s);return 1;",
        Arc::new(Host::default()),
        Limits {
            max_log_bytes: 0,
            max_data_bytes: 100_000,
            ..Limits::default()
        },
        cancel(),
    )
    .await;
    assert!(r.ok, "{:?}", r.error);
    assert!(r.logs.is_empty());
    assert!(r.truncated);
    assert_eq!(value(run("const x=Object.fromEntries([['k'.repeat(10000),1]]);const alias=x;x.a=2;return alias.a;").await),json!(2));
    let r = execute(
        "return ' a '.repeat(10000).trim();",
        Arc::new(Host::default()),
        Limits {
            max_data_bytes: 100_000,
            ..Limits::default()
        },
        cancel(),
    )
    .await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().kind, "limit");
}
#[tokio::test]
async fn diagnostic_byte_zero_and_catch_origin_are_preserved() {
    let result = run("missing();").await;
    let error = result.error.unwrap();
    assert_eq!(error.line, Some(1));
    assert_eq!(error.column, Some(1));
    assert_eq!(value(run("try {\n  await tools.test.fail({});\n}catch(e){return {line:e.line,column:e.column,kind:e.kind};}").await),json!({"line":2,"column":3,"kind":"tool"}));
}

#[tokio::test]
async fn assignments_freeze_reference_and_read_compound_value_before_rhs() {
    assert_eq!(value(run("let key='a';const x={a:1,b:7};function rhs(){key='b';x.a=9;return 2;}x[key]+=rhs();return x;").await),json!({"a":3,"b":7}));
    assert_eq!(value(run("let x={a:1};const old=x;function rhs(){x={a:2};return 3;}x.a=rhs();return [old.a,x.a];").await),json!([3,2]));
}
#[tokio::test]
async fn functions_keep_distinct_identity_and_keyword_data_keys_remain_valid() {
    assert_eq!(value(run("const f=()=>1;const alias=f;function make(){return ()=>1;}return [f===alias,make()===make()];").await),json!([true,false]));
    assert_eq!(
        value(run("return await tools.test.delete({x:1});").await),
        json!({"x":1})
    );
    assert_eq!(
        value(run("return {with:1,delete:2,new:3};").await),
        json!({"with":1,"delete":2,"new":3})
    );
}
// A stack overflow terminates a process, not a Rust catch_unwind. Keep dangerous
// parser regression cases in a subprocess so the main suite remains diagnosable.
#[test]
fn parser_stack_hazards_child() {
    if std::env::var_os("NEOISM_COMPOSITION_PARSER_CHILD").is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for code in [
            format!("{};", "with(a,b)".repeat(6000)),
            format!("{};while(a);", "do ".repeat(6000)),
            format!("{};", "new(a,b)".repeat(6000)),
            format!("{};", "if(a,b)".repeat(6000)),
            format!("{};", "a:".repeat(10000)),
            format!("if(a) /'/;{};", "with(a,b)".repeat(5000)),
            format!("{};", r"w\u0069th(a,b)".repeat(4000)),
            format!(
                "{}1{};",
                format!("{}(a,", "2**".repeat(150)).repeat(46),
                ")".repeat(46)
            ),
            format!(
                "{}1{};",
                format!("{}(a,", "await ".repeat(70)).repeat(46),
                ")".repeat(46)
            ),
            format!(
                "{}1{};",
                format!("{}(a,", "typeof ".repeat(60)).repeat(46),
                ")".repeat(46)
            ),
        ] {
            let r = run(&code).await;
            assert!(!r.ok);
            assert!(r.tool_calls.is_empty());
        }
        // Guards must not misinterpret comments/quoted payloads as introducers.
        assert_eq!(
            value(run("const s='with(a,b)';/* with(a,b) */ return 12/(1+2);").await),
            json!(4)
        );
    });
}
#[test]
fn parser_stack_hazards_are_isolated() {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "parser_stack_hazards_child", "--nocapture"])
        .env("NEOISM_COMPOSITION_PARSER_CHILD", "1")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "parser child exited {status}");
            break;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("parser child timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
