use futures::{future::BoxFuture, FutureExt};
use neoism_agent_composition::*;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

#[derive(Default)]
struct Host {
    calls: AtomicUsize,
}
impl ToolHost for Host {
    fn catalog(&self) -> Vec<ToolDescriptor> {
        ["test.echo", "test.string", "test.fail", "test.push"]
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
        _: Arc<AtomicBool>,
    ) -> BoxFuture<'static, Result<Value, String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        async move {
            match path.as_str() {
                "test.string" => Ok(json!("dynamic")),
                "test.fail" => Err("host failed".into()),
                _ => Ok(input),
            }
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
async fn scalar_index_of_unicode_offsets_empty_and_absent() {
    assert_eq!(value(run("const s='a🎉bé🎉';return [s.length,s.indexOf('🎉'),s.indexOf('🎉',2),s.indexOf('é🎉'),s.indexOf('missing'),s.indexOf('🎉',99),s.indexOf('a',-3),s.indexOf('🎉',1.9)];").await),json!([5,1,4,3,-1,-1,0,1]));
    assert_eq!(value(run("return ['a🎉b'.indexOf(''), 'a🎉b'.indexOf('',1), 'a🎉b'.indexOf('',100), ''.indexOf(''), ''.indexOf('x'), 'abc'.indexOf('a',1/0), 'abc'.indexOf('',1/0), 'abc'.indexOf('a',-1/0), 'abc'.indexOf('a',0/0)];").await),json!([0,1,3,0,-1,-1,3,0,0]));
    assert_eq!(value(run("return ['x123x'.indexOf(123),'undefined'.indexOf(),'null'.indexOf(null),'false'.indexOf(false),'xxaxx'.indexOf('a','2.9'),'xxaxx'.indexOf('a','no-number'),'xxaxx'.indexOf('a','')];").await),json!([1,0,0,0,2,2,2]));
    assert_eq!(
        value(run("return 'ababababac'.indexOf('ababac');").await),
        json!(4)
    );
}
#[tokio::test]
async fn index_of_has_fuel_data_and_timeout_bounds() {
    let host = Arc::new(Host::default());
    for limits in [
        Limits {
            max_steps: 300,
            ..Limits::default()
        },
        Limits {
            wall_clock: std::time::Duration::ZERO,
            ..Limits::default()
        },
    ] {
        let r = execute(
            "return 'a'.repeat(10000).indexOf('b');",
            host.clone(),
            limits,
            cancel(),
        )
        .await;
        assert!(!r.ok);
        assert!(matches!(
            r.error.unwrap().kind.as_str(),
            "limit" | "timeout"
        ));
    }
    let r = execute(
        "const needle='a'.repeat(2000);return 'a'.indexOf(needle);",
        host,
        Limits {
            max_data_bytes: 20_000,
            ..Limits::default()
        },
        cancel(),
    )
    .await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().kind, "limit");
}
#[tokio::test]
async fn push_appends_atomically_to_const_arrays_and_aliases() {
    assert_eq!(value(run("const a=[1];const alias=a;const read=()=>alias;const n=a.push(2,3);return {n,z:a.push(),a,alias:read(),same:a===alias};").await),json!({"n":3,"z":3,"a":[1,2,3],"alias":[1,2,3],"same":true}));
    assert_eq!(value(run("const a=[];const f=async x=>{await tools.test.echo({x});return a.push(x);};const lengths=await Promise.all([1,2,3].map(x=>f(x)));return {a,lengths};").await)["a"].as_array().unwrap().len(),3);
    assert_eq!(value(run("const a=[1];try{a.push(2,a);}catch(e){return {a,message:String(e),kind:e.kind};}return a;").await)["a"],json!([1]));
    assert_eq!(value(run("const a=[];const parent={a};try{a.push(1,parent);}catch(e){return {a,kind:e.kind};}return 0;").await),json!({"a":[],"kind":"security"}));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_push_returns_its_own_new_length() {
    let r=value(run("const a=[];return await Promise.all([0,1,2,3,4,5,6,7].map(async x=>a.push(x))); ").await);
    let mut lengths: Vec<u64> = r
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap())
        .collect();
    lengths.sort_unstable();
    assert_eq!(lengths, (1..=8).collect::<Vec<_>>());
}
#[tokio::test]
async fn caught_error_text_keeps_json_origin_and_primitive_coercion() {
    let r=value(run("try {\n await tools.test.fail({});\n}catch(e){return {text:String(e),concat:'caught '+e,error:e};}").await);
    assert_eq!(r["text"], "tool: host failed");
    assert_eq!(r["concat"], "caught tool: host failed");
    assert_eq!(r["error"]["message"], "host failed");
    assert_eq!(r["error"]["kind"], "tool");
    assert_eq!(r["error"]["line"], 2);
    assert_eq!(r["error"]["column"], 2);
    assert_eq!(value(run("return [String(),String(undefined),String(null),String(true),String(false),String(12),String(-0),String(1/0),String(-1/0),String(0/0),String('text'),String({kind:'user',message:'not caught'})];").await),json!(["","undefined","null","true","false","12","0","Infinity","-Infinity","NaN","text","[object Object]"]));
    assert_eq!(value(run("return [String(5e-324),String(1e-7),String(Number('1e21'))];").await),json!(["5e-324","1e-7","1e+21"]));
    let host = Arc::new(Host::default());
    let r = execute(
        "const x={toString:()=>tools.test.echo({})};return String(x);",
        host.clone(),
        Limits::default(),
        cancel(),
    )
    .await;
    assert_eq!(value(r), json!("[object Object]"));
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        value(run("try{throw 'x';}catch(e){console.log(e);return String(e);}").await),
        json!("throw: x")
    );
}
#[tokio::test]
async fn static_unsupported_members_fail_before_any_tools() {
    for suffix in [
        "'x'.noSuchMethod();",
        "[].noSuchMethod();",
        "JSON.noSuchMethod();",
        "Math.random();",
        "Object.create({});",
        "String.raw('x');",
        "Number.isNaN(0);",
        "const s='x';s.noSuchMethod();",
        "let s='x';s.noSuchMethod();",
        "const a=[];a.pop();",
        "const j=JSON;j.noSuchMethod();",
        "'x'.length();",
    ] {
        let host = Arc::new(Host::default());
        let r = execute(
            &format!("await tools.test.echo({{}});{suffix}"),
            host.clone(),
            Limits::default(),
            cancel(),
        )
        .await;
        assert!(!r.ok, "{suffix}");
        assert_eq!(host.calls.load(Ordering::SeqCst), 0, "{suffix}");
        assert!(r.tool_calls.is_empty());
    }
    for prefix in ["const s='x';", "let s='x';"] {
        let host = Arc::new(Host::default());
        let r = execute(
            &format!("{prefix}await tools.test.echo({{}});s.noSuchMethod();"),
            host.clone(),
            Limits::default(),
            cancel(),
        )
        .await;
        assert!(!r.ok);
        assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    }
    let host = Arc::new(Host::default());
    let r = execute(
        "await tools.test.echo({});const a=[];a.push('x');return a[0].indexOf('x');",
        host.clone(),
        Limits::default(),
        cancel(),
    )
    .await;
    assert_eq!(value(r), json!(0));
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn dynamic_failure_is_not_transactional_and_custom_names_stay_allowed() {
    let host = Arc::new(Host::default());
    let r=execute("const s=await tools.test.string({});await tools.test.echo({});return s.noSuchMethod();",host.clone(),Limits::default(),cancel()).await;
    assert!(!r.ok);
    assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    assert_eq!(r.tool_calls.len(), 2);
    assert!(r.tool_calls.iter().all(|call| call.status == "completed"));
    assert_eq!(value(run("const x={noSuchMethod:()=>7,push:x=>x+1,indexOf:()=>8};return [x.noSuchMethod(),x.push(2),x.indexOf(),await tools.test.push({a:1})];").await),json!([7,3,8,{"a":1}]));
    assert_eq!(value(run("let s='x';function change(){s={noSuchMethod:()=>9};}change();await tools.test.echo({});return s.noSuchMethod();").await),json!(9));
    assert_eq!(value(run("let s='x';s={noSuchMethod:()=>10};await tools.test.echo({});return s.noSuchMethod();").await),json!(10));
}
