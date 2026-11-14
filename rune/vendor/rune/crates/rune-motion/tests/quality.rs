use rune_motion::{Scene, NodeId, NodeSpec, Rect, Mount, Spring};

#[test]
fn jittered_sampling_and_retarget_audit_stay_bounded() {
    let mut scene = Scene::new();
    scene.store_mut().audit_retargets(true);
    scene.insert(NodeId(1), NodeSpec::new(Rect::new(0.0, 0.0, 200.0, 200.0)), Mount::Initial);
    scene.insert(NodeId(2), NodeSpec::new(Rect::new(10.0, 10.0, 50.0, 50.0)).parent(NodeId(1)), Mount::Initial);
    scene.frame_at(0.0);
    let mut now = 0.0;
    let mut seed = 7u32;
    for i in 0..300 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        now += 1.0 / 120.0 + (seed % 12000) as f64 / 1e6;
        scene.set_time(now);
        if i % 7 == 0 {
            let x = (seed % 300) as f32;
            scene.set_frame(NodeId(1), Rect::new(x, x / 2.0, 200.0, 200.0), Spring::new(0.4, 0.0));
            scene.set_frame(NodeId(2), Rect::new(x / 4.0, x / 3.0, 50.0, 50.0), Spring::new(0.3, 0.0));
        }
        scene.frame();
        let (samples, error) = scene.sampling_error();
        assert_eq!(samples, 8);
        assert!(error < 0.016, "analytic edge error {error}px at {now}");
    }
    let audit = scene.store_mut().take_retarget_audit();
    assert!(audit.lanes > 100);
    assert!(audit.max_velocity_error < 0.001, "{audit:?}");
}
