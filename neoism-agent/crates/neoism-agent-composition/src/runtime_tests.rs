//! Private-state regressions verify failed admission leaves the actual shared
//! container unchanged even when fatal limits prevent a script inspecting it.
use super::*;
struct NoHost;
impl ToolHost for NoHost {
    fn catalog(&self) -> Vec<ToolDescriptor> {
        vec![]
    }
    fn call(
        &self,
        _: String,
        _: Value,
        _: Arc<AtomicBool>,
    ) -> BoxFuture<'static, Result<Value, String>> {
        Box::pin(async { Err("unused host".into()) })
    }
}
fn engine() -> Engine {
    Engine {
        source: Arc::from(""),
        host: Arc::new(NoHost),
        catalog: Arc::new(vec![]),
        limits: Arc::new(Limits::default()),
        local: Arc::new(AtomicBool::new(false)),
        external: Arc::new(AtomicBool::new(false)),
        deadline: Instant::now() + Duration::from_secs(120),
        semaphore: Arc::new(Semaphore::new(8)),
        state: Arc::new(Mutex::new(State {
            fatal: None,
            function_ids: 0,
            arrays: vec![],
            caught_errors: std::collections::BTreeSet::new(),
            objects: vec![],
            envs: vec![Env {
                parent: None,
                vars: BTreeMap::new(),
            }],
            steps: 0,
            yield_at: 128,
            allocated: 0,
            logs: vec![],
            log_bytes: 0,
            records: vec![],
            promises: vec![],
            tasks: vec![],
            aborts: vec![],
            truncated: false,
        })),
    }
}
fn array_id(value: V) -> usize {
    let V::Array(id) = value else {
        panic!("expected array")
    };
    id
}
fn object_id(value: V) -> usize {
    let V::Object(id) = value else {
        panic!("expected object")
    };
    id
}
#[test]
fn push_cycle_rejection_is_all_or_nothing() {
    let engine = engine();
    let id = array_id(engine.array(vec![V::Num(1.0)]).unwrap());
    let capacity = engine.state.lock().unwrap().arrays[id].capacity();
    let error = engine
        .mutate(Mutation::Push(id, vec![V::Num(2.0), V::Array(id)]))
        .unwrap_err();
    assert_eq!(error.kind, "security");
    let state = engine.state.lock().unwrap();
    assert_eq!(state.arrays[id].len(), 1);
    assert_eq!(state.arrays[id].capacity(), capacity);
    assert!(matches!(state.arrays[id][0], V::Num(1.0)));
}
#[test]
fn push_data_fuel_deadline_and_cancel_failures_do_not_allocate_or_append() {
    for failure in ["data", "fuel", "timeout", "cancel"] {
        let mut engine = engine();
        let id = array_id(engine.array(vec![V::Num(1.0)]).unwrap());
        let mut limits = Limits::default();
        match failure {
            "data" => limits.max_data_bytes = engine.state.lock().unwrap().allocated + 64,
            "fuel" => limits.max_steps = engine.state.lock().unwrap().steps + 1,
            "timeout" => engine.deadline = Instant::now(),
            "cancel" => engine.external.store(true, Ordering::Relaxed),
            _ => unreachable!(),
        }
        engine.limits = Arc::new(limits);
        let capacity = engine.state.lock().unwrap().arrays[id].capacity();
        let error = engine
            .mutate(Mutation::Push(id, vec![V::Num(2.0), V::Num(3.0)]))
            .unwrap_err();
        assert!(error.fatal);
        let state = engine.state.lock().unwrap();
        assert_eq!(state.arrays[id].len(), 1, "{failure}");
        assert_eq!(state.arrays[id].capacity(), capacity, "{failure}");
        assert!(matches!(state.arrays[id][0], V::Num(1.0)));
    }
}
#[test]
fn push_validates_depth_from_all_retained_parents_before_appending() {
    let mut engine = engine();
    let id = array_id(engine.array(vec![V::Num(1.0)]).unwrap());
    let parent = engine.array(vec![V::Array(id)]).unwrap();
    engine.array(vec![parent]).unwrap();
    let deeper = engine.array(vec![V::Num(3.0)]).unwrap();
    engine.limits = Arc::new(Limits {
        max_depth: 4,
        ..Limits::default()
    });
    let capacity = engine.state.lock().unwrap().arrays[id].capacity();
    assert_eq!(
        engine
            .mutate(Mutation::Push(id, vec![V::Num(2.0), deeper]))
            .unwrap_err()
            .kind,
        "limit"
    );
    let state = engine.state.lock().unwrap();
    assert_eq!(state.arrays[id].len(), 1);
    assert_eq!(state.arrays[id].capacity(), capacity);
}
#[test]
fn aliases_share_string_handle_and_empty_policy_has_zero_extra_admission() {
    static ALIASES: &[ResultAlias] = &[
        ResultAlias {
            name: "data",
            path: &["nested", "payload"],
        },
        ResultAlias {
            name: "other",
            path: &["nested", "payload"],
        },
    ];
    let mut engine = engine();
    let value = engine
        .from_json(
            &serde_json::json!({"nested":{"payload":"x".repeat(100000)}}),
            0,
        )
        .unwrap();
    let root = object_id(value.clone());
    let before = engine.state.lock().unwrap().allocated;
    engine.limits = Arc::new(Limits {
        max_data_bytes: before + 2048,
        ..Limits::default()
    });
    let value = engine.result_aliases(value, ALIASES).unwrap();
    let state = engine.state.lock().unwrap();
    let V::Object(nested) = state.objects[root]["nested"] else {
        panic!()
    };
    let (V::Str(canonical), V::Str(data), V::Str(other)) = (
        &state.objects[nested]["payload"],
        &state.objects[root]["data"],
        &state.objects[root]["other"],
    ) else {
        panic!()
    };
    assert!(Arc::ptr_eq(canonical, data));
    assert!(Arc::ptr_eq(data, other));
    assert!(state.allocated - before < 2048);
    let allocated = state.allocated;
    let steps = state.steps;
    drop(state);
    engine.result_aliases(value, &[]).unwrap();
    let state = engine.state.lock().unwrap();
    assert_eq!(state.allocated, allocated);
    assert_eq!(state.steps, steps);
}
#[test]
fn alias_key_growth_is_admitted_before_insert() {
    static ALIASES: &[ResultAlias] = &[ResultAlias {
        name: "data",
        path: &["payload"],
    }];
    let mut engine = engine();
    let value = engine
        .from_json(&serde_json::json!({"payload":1}), 0)
        .unwrap();
    let root = object_id(value.clone());
    let before = engine.state.lock().unwrap().allocated;
    engine.limits = Arc::new(Limits {
        max_data_bytes: before + 68,
        ..Limits::default()
    });
    assert_eq!(
        engine.result_aliases(value, ALIASES).unwrap_err().kind,
        "limit"
    );
    assert!(!engine.state.lock().unwrap().objects[root].contains_key("data"));
}
#[tokio::test]
async fn index_of_cooperative_checkpoints_observe_current_thread_cancellation() {
    let engine = engine();
    let flag = engine.external.clone();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        flag.store(true, Ordering::Relaxed);
    });
    let error = engine
        .string_index_of(&"a".repeat(50000), "b", &V::Num(0.0), 0)
        .await
        .unwrap_err();
    assert_eq!(error.kind, "cancelled");
}
