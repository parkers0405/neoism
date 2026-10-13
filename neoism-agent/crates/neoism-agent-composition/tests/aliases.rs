use futures::{future::BoxFuture, FutureExt};
use neoism_agent_composition::*;
use serde_json::{json, Value};
use std::sync::{atomic::AtomicBool, Arc};

static ALIASES: &[ResultAlias] = &[
    ResultAlias {
        name: "data",
        path: &["canonical", "payload"],
    },
    ResultAlias {
        name: "structuredContent",
        path: &["canonical", "payload"],
    },
];
struct Host {
    value: Value,
    aliases: &'static [ResultAlias],
}
impl ToolHost for Host {
    fn catalog(&self) -> Vec<ToolDescriptor> {
        vec![ToolDescriptor {
            path: "test.result".into(),
            description: String::new(),
            signature: "() => JSON".into(),
        }]
    }
    fn result_aliases(&self, _: &str) -> &'static [ResultAlias] {
        self.aliases
    }
    fn call(
        &self,
        _: String,
        _: Value,
        _: Arc<AtomicBool>,
    ) -> BoxFuture<'static, Result<Value, String>> {
        let value = self.value.clone();
        async move { Ok(value) }.boxed()
    }
}
async fn run(
    code: &str,
    value: Value,
    aliases: &'static [ResultAlias],
    limits: Limits,
) -> ExecutionResult {
    execute(
        code,
        Arc::new(Host { value, aliases }),
        limits,
        Arc::new(AtomicBool::new(false)),
    )
    .await
}
#[tokio::test]
async fn result_aliases_share_existing_arena_and_serialize_selected_views() {
    let source = json!({"canonical":{"payload":[{"n":1}]},"status":"ok"});
    let host = Arc::new(Host {
        value: source.clone(),
        aliases: ALIASES,
    });
    let result=execute("const r=await tools.test.result({});r.data[0].n=4;const n=r.structuredContent.push({n:5});return {n,same:r.data===r.canonical.payload,selected:r.structuredContent,status:r.status};",host.clone(),Limits::default(),Arc::new(AtomicBool::new(false))).await;
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(
        result.value,
        json!({"n":2,"same":true,"selected":[{"n":4},{"n":5}],"status":"ok"})
    );
    assert_eq!(
        host.value, source,
        "interpreter must not mutate external host JSON"
    );
    let result = run(
        "return await tools.test.result({});",
        json!({"canonical":{"payload":[1,2]}}),
        ALIASES,
        Limits::default(),
    )
    .await;
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(
        result.value,
        json!({"canonical":{"payload":[1,2]},"data":[1,2],"structuredContent":[1,2]})
    );
}
#[tokio::test]
async fn large_payload_is_imported_once_not_once_per_alias() {
    let payload = "x".repeat(262_144);
    let result=run("const r=await tools.test.result({});return r.data===r.canonical.payload&&r.structuredContent===r.data;",json!({"canonical":{"payload":payload}}),ALIASES,Limits{max_data_bytes:300_000,..Limits::default()}).await;
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.value, json!(true));
}
#[tokio::test]
async fn missing_targets_are_skipped_and_empty_default_needs_no_object() {
    let result = run(
        "return await tools.test.result({});",
        json!({"canonical":{"other":1}}),
        ALIASES,
        Limits::default(),
    )
    .await;
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.value, json!({"canonical":{"other":1}}));
    let result = run(
        "return await tools.test.result({});",
        json!([1, 2]),
        &[],
        Limits::default(),
    )
    .await;
    assert!(result.ok);
    assert_eq!(result.value, json!([1, 2]));
}
#[tokio::test]
async fn aliases_reject_prototypes_conflicts_duplicates_cycles_and_bad_roots() {
    static FORBIDDEN_NAME: &[ResultAlias] = &[ResultAlias {
        name: "__proto__",
        path: &["canonical"],
    }];
    static FORBIDDEN_PATH: &[ResultAlias] = &[ResultAlias {
        name: "data",
        path: &["constructor"],
    }];
    static DUPLICATE: &[ResultAlias] = &[
        ResultAlias {
            name: "data",
            path: &["canonical"],
        },
        ResultAlias {
            name: "data",
            path: &["canonical"],
        },
    ];
    static CYCLE: &[ResultAlias] = &[ResultAlias {
        name: "self",
        path: &[],
    }];
    for aliases in [FORBIDDEN_NAME, FORBIDDEN_PATH, DUPLICATE, CYCLE] {
        let result = run(
            "return await tools.test.result({});",
            json!({"canonical":{"payload":1}}),
            aliases,
            Limits::default(),
        )
        .await;
        assert!(!result.ok);
        assert!(matches!(
            result.error.unwrap().kind.as_str(),
            "security" | "alias"
        ));
        assert_eq!(result.tool_calls[0].status, "error");
    }
    let result = run(
        "return await tools.test.result({});",
        json!({"canonical":{"payload":1},"data":2}),
        ALIASES,
        Limits::default(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().kind, "alias");
    let result = run(
        "return await tools.test.result({});",
        json!([1]),
        ALIASES,
        Limits::default(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().kind, "type");
}
#[tokio::test]
async fn alias_count_depth_and_data_limits_are_enforced() {
    static TOO_MANY: [ResultAlias; 33] = [ResultAlias {
        name: "a",
        path: &["canonical"],
    }; 33];
    static DEEP_PATH: [&str; 64] = ["nested"; 64];
    static DEEP: &[ResultAlias] = &[ResultAlias {
        name: "data",
        path: &DEEP_PATH,
    }];
    for aliases in [&TOO_MANY[..], DEEP] {
        let result = run(
            "return await tools.test.result({});",
            json!({"canonical":{"payload":1}}),
            aliases,
            Limits::default(),
        )
        .await;
        assert!(!result.ok);
        assert_eq!(result.error.unwrap().kind, "limit");
    }
    let result = run(
        "return await tools.test.result({});",
        json!({"canonical":{"payload":"x".repeat(20000)}}),
        ALIASES,
        Limits {
            max_data_bytes: 10_000,
            ..Limits::default()
        },
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().kind, "limit");
}
