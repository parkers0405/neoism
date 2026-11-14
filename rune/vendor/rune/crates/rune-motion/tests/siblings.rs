use rune_motion::{Scene, NodeId, NodeSpec, Rect, Mount};

#[test]
fn sibling_order_matches_reference_after_removal_and_slot_reuse() {
    for nested in [false, true] {
        let mut scene = Scene::new();
        let parent = NodeId(1000);
        if nested { scene.insert(parent, NodeSpec::new(Rect::ZERO), Mount::Initial); }
        let mut expected = Vec::new();
        let mut seed = 1u32;
        for step in 0..300u64 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            if !expected.is_empty() && !seed.is_multiple_of(3) {
                let i = seed as usize % expected.len();
                scene.unmount(expected.remove(i));
            } else {
                let id = NodeId(step);
                let spec = NodeSpec::new(Rect::new(0.0, 0.0, 10.0, 10.0));
                scene.insert(id, if nested { spec.parent(parent) } else { spec }, Mount::Initial);
                expected.push(id);
            }
            // Force every surviving node into the changed list to observe traversal order.
            for &id in &expected { scene.set(id, rune_motion::prop::OPACITY, if step % 2 == 0 { 0.8 } else { 1.0 }); }
            let out = scene.frame_at(step as f64 / 120.0);
            let actual: Vec<_> = out.changed.iter().filter(|p| p.node != parent).map(|p| p.node).collect();
            assert_eq!(actual, expected, "step {step}, nested {nested}");
        }
    }
}
