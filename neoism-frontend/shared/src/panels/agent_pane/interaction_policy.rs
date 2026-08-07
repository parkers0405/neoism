use std::collections::HashMap;

pub type HitRect = (String, [f32; 4]);
pub type DiffScrollRect = (String, [f32; 4], f32);

pub fn rect_contains(rect: [f32; 4], x: f32, y: f32) -> bool {
    x >= rect[0] && x <= rect[0] + rect[2] && y >= rect[1] && y <= rect[1] + rect[3]
}

pub fn register_hit_rect(rects: &mut Vec<HitRect>, id: String, rect: [f32; 4]) {
    if !id.is_empty() {
        rects.push((id, rect));
    }
}

pub fn hit_rect_target(rects: &[HitRect], x: f32, y: f32) -> Option<(String, [f32; 4])> {
    rects
        .iter()
        .rev()
        .find(|(_, rect)| rect_contains(*rect, x, y))
        .map(|(target, rect)| (target.clone(), *rect))
}

pub fn register_diff_scroll_rect(
    rects: &mut Vec<DiffScrollRect>,
    key: String,
    rect: [f32; 4],
    max_scroll: f32,
) {
    if !key.is_empty() && max_scroll > 1.0 {
        rects.push((key, rect, max_scroll));
    }
}

pub fn diff_scroll_offset(
    offsets: &mut HashMap<String, f32>,
    key: &str,
    max_scroll: f32,
) -> f32 {
    if max_scroll <= 1.0 {
        offsets.remove(key);
        return 0.0;
    }
    let offset = offsets.entry(key.to_string()).or_insert(0.0);
    *offset = (*offset).clamp(0.0, max_scroll);
    *offset
}

pub fn scroll_diff_at(
    rects: &[DiffScrollRect],
    offsets: &mut HashMap<String, f32>,
    x: f32,
    y: f32,
    delta_pixels: f32,
) -> Option<bool> {
    let (key, _, max_scroll) = rects
        .iter()
        .rev()
        .find(|(_, rect, _)| rect_contains(*rect, x, y))
        .cloned()?;
    let offset = offsets.entry(key).or_insert(0.0);
    let next = (*offset + delta_pixels).clamp(0.0, max_scroll);
    if (next - *offset).abs() < f32::EPSILON {
        return Some(false);
    }
    *offset = next;
    Some(true)
}

pub fn update_hover_target(current: &mut Option<String>, next: Option<String>) -> bool {
    if *current == next {
        return false;
    }
    *current = next;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_rect_target_prefers_latest_registered_rect() {
        let mut rects = Vec::new();
        register_hit_rect(&mut rects, "old".to_string(), [0.0, 0.0, 20.0, 20.0]);
        register_hit_rect(&mut rects, "new".to_string(), [10.0, 10.0, 20.0, 20.0]);
        register_hit_rect(&mut rects, String::new(), [0.0, 0.0, 100.0, 100.0]);

        assert_eq!(
            hit_rect_target(&rects, 12.0, 12.0),
            Some(("new".to_string(), [10.0, 10.0, 20.0, 20.0]))
        );
        assert_eq!(
            hit_rect_target(&rects, 5.0, 5.0),
            Some(("old".to_string(), [0.0, 0.0, 20.0, 20.0]))
        );
        assert_eq!(hit_rect_target(&rects, 40.0, 40.0), None);
    }

    #[test]
    fn diff_scroll_offsets_clamp_and_prune_inactive_scroll_regions() {
        let mut offsets =
            HashMap::from([("diff".to_string(), 20.0), ("tiny".to_string(), 3.0)]);

        assert_eq!(diff_scroll_offset(&mut offsets, "diff", 10.0), 10.0);
        assert_eq!(diff_scroll_offset(&mut offsets, "tiny", 1.0), 0.0);
        assert!(!offsets.contains_key("tiny"));
    }

    #[test]
    fn diff_scroll_at_uses_topmost_hit_rect_and_reports_edge_hits() {
        let rects = vec![
            ("lower".to_string(), [0.0, 0.0, 30.0, 30.0], 100.0),
            ("upper".to_string(), [10.0, 10.0, 30.0, 30.0], 25.0),
        ];
        let mut offsets = HashMap::new();

        assert_eq!(
            scroll_diff_at(&rects, &mut offsets, 12.0, 12.0, 10.0),
            Some(true)
        );
        assert_eq!(offsets.get("upper"), Some(&10.0));
        assert_eq!(
            scroll_diff_at(&rects, &mut offsets, 12.0, 12.0, 30.0),
            Some(true)
        );
        assert_eq!(offsets.get("upper"), Some(&25.0));
        assert_eq!(
            scroll_diff_at(&rects, &mut offsets, 12.0, 12.0, 1.0),
            Some(false)
        );
        assert_eq!(
            scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, 1.0),
            None
        );
    }

    #[test]
    fn long_diff_scroll_walks_the_entire_internal_range() {
        let rects = vec![(
            "tool-message:0".to_string(),
            [20.0, 40.0, 640.0, 220.0],
            2_400.0,
        )];
        let mut offsets = HashMap::new();

        for step in 1..=24 {
            assert_eq!(
                scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, 100.0),
                Some(true)
            );
            assert_eq!(
                offsets.get("tool-message:0").copied(),
                Some(step as f32 * 100.0)
            );
        }

        assert_eq!(
            scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, 100.0),
            Some(false)
        );
        assert_eq!(offsets.get("tool-message:0"), Some(&2_400.0));
    }

    #[test]
    fn long_diff_scroll_can_reverse_back_to_the_first_line() {
        let rects = vec![(
            "tool-message:0".to_string(),
            [20.0, 40.0, 640.0, 220.0],
            3_000.0,
        )];
        let mut offsets = HashMap::from([("tool-message:0".to_string(), 3_000.0)]);

        for step in (0..30).rev() {
            assert_eq!(
                scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, -100.0),
                Some(true)
            );
            assert_eq!(
                offsets.get("tool-message:0").copied(),
                Some(step as f32 * 100.0)
            );
        }

        assert_eq!(
            scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, -100.0),
            Some(false)
        );
        assert_eq!(offsets.get("tool-message:0"), Some(&0.0));
    }

    #[test]
    fn long_diff_scroll_clamps_large_wheel_impulses() {
        let rects = vec![(
            "tool-message:0".to_string(),
            [20.0, 40.0, 640.0, 220.0],
            1_750.0,
        )];
        let mut offsets = HashMap::new();

        assert_eq!(
            scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, 10_000.0),
            Some(true)
        );
        assert_eq!(offsets.get("tool-message:0"), Some(&1_750.0));

        assert_eq!(
            scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, -10_000.0),
            Some(true)
        );
        assert_eq!(offsets.get("tool-message:0"), Some(&0.0));
    }

    #[test]
    fn separate_diff_files_keep_independent_scroll_positions() {
        let rects = vec![
            (
                "tool-message:0".to_string(),
                [20.0, 40.0, 640.0, 220.0],
                2_000.0,
            ),
            (
                "tool-message:1".to_string(),
                [20.0, 280.0, 640.0, 220.0],
                4_000.0,
            ),
            (
                "tool-message:2".to_string(),
                [20.0, 520.0, 640.0, 220.0],
                6_000.0,
            ),
        ];
        let mut offsets = HashMap::new();

        assert_eq!(
            scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, 350.0),
            Some(true)
        );
        assert_eq!(
            scroll_diff_at(&rects, &mut offsets, 100.0, 340.0, 700.0),
            Some(true)
        );
        assert_eq!(
            scroll_diff_at(&rects, &mut offsets, 100.0, 580.0, 1_050.0),
            Some(true)
        );

        assert_eq!(offsets.get("tool-message:0"), Some(&350.0));
        assert_eq!(offsets.get("tool-message:1"), Some(&700.0));
        assert_eq!(offsets.get("tool-message:2"), Some(&1_050.0));
    }

    #[test]
    fn scrolling_one_diff_does_not_disturb_another_diff() {
        let rects = vec![
            (
                "tool-message:0".to_string(),
                [20.0, 40.0, 640.0, 220.0],
                2_000.0,
            ),
            (
                "tool-message:1".to_string(),
                [20.0, 280.0, 640.0, 220.0],
                2_000.0,
            ),
        ];
        let mut offsets = HashMap::from([
            ("tool-message:0".to_string(), 500.0),
            ("tool-message:1".to_string(), 900.0),
        ]);

        for _ in 0..10 {
            assert_eq!(
                scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, 50.0),
                Some(true)
            );
        }

        assert_eq!(offsets.get("tool-message:0"), Some(&1_000.0));
        assert_eq!(offsets.get("tool-message:1"), Some(&900.0));
    }

    #[test]
    fn pointer_outside_a_diff_leaves_every_internal_offset_unchanged() {
        let rects = vec![
            (
                "tool-message:0".to_string(),
                [20.0, 40.0, 640.0, 220.0],
                2_000.0,
            ),
            (
                "tool-message:1".to_string(),
                [20.0, 280.0, 640.0, 220.0],
                2_000.0,
            ),
        ];
        let original = HashMap::from([
            ("tool-message:0".to_string(), 500.0),
            ("tool-message:1".to_string(), 900.0),
        ]);
        let mut offsets = original.clone();

        for (x, y) in [
            (0.0, 0.0),
            (10.0, 100.0),
            (700.0, 100.0),
            (100.0, 270.0),
            (100.0, 510.0),
            (1_000.0, 1_000.0),
        ] {
            assert_eq!(scroll_diff_at(&rects, &mut offsets, x, y, 100.0), None);
        }

        assert_eq!(offsets, original);
    }

    #[test]
    fn topmost_overlapping_diff_receives_the_entire_wheel_sequence() {
        let rects = vec![
            (
                "underneath".to_string(),
                [20.0, 40.0, 640.0, 220.0],
                2_000.0,
            ),
            ("topmost".to_string(), [60.0, 80.0, 560.0, 140.0], 3_000.0),
        ];
        let mut offsets = HashMap::new();

        for _ in 0..20 {
            assert_eq!(
                scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, 75.0),
                Some(true)
            );
        }

        assert_eq!(offsets.get("topmost"), Some(&1_500.0));
        assert!(!offsets.contains_key("underneath"));
    }

    #[test]
    fn exhausted_diff_reports_edge_hits_for_timeline_scroll_bubbling() {
        let rects = vec![(
            "tool-message:0".to_string(),
            [20.0, 40.0, 640.0, 220.0],
            1_000.0,
        )];
        let mut offsets = HashMap::from([("tool-message:0".to_string(), 1_000.0)]);

        for delta in [1.0, 10.0, 100.0, 1_000.0] {
            assert_eq!(
                scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, delta),
                Some(false)
            );
            assert_eq!(offsets.get("tool-message:0"), Some(&1_000.0));
        }

        offsets.insert("tool-message:0".to_string(), 0.0);
        for delta in [-1.0, -10.0, -100.0, -1_000.0] {
            assert_eq!(
                scroll_diff_at(&rects, &mut offsets, 100.0, 100.0, delta),
                Some(false)
            );
            assert_eq!(offsets.get("tool-message:0"), Some(&0.0));
        }
    }

    #[test]
    fn inactive_diff_offsets_are_pruned_when_the_card_has_no_overflow() {
        let mut offsets = HashMap::from([
            ("active".to_string(), 450.0),
            ("inactive".to_string(), 450.0),
            ("missing".to_string(), 450.0),
        ]);

        assert_eq!(diff_scroll_offset(&mut offsets, "active", 1_000.0), 450.0);
        assert_eq!(diff_scroll_offset(&mut offsets, "inactive", 0.0), 0.0);
        assert_eq!(diff_scroll_offset(&mut offsets, "missing", -100.0), 0.0);

        assert_eq!(offsets.get("active"), Some(&450.0));
        assert!(!offsets.contains_key("inactive"));
        assert!(!offsets.contains_key("missing"));
    }

    #[test]
    fn reopening_a_shorter_diff_clamps_its_previous_scroll_position() {
        let mut offsets = HashMap::from([("tool-message:0".to_string(), 2_500.0)]);

        let first_reopen = diff_scroll_offset(&mut offsets, "tool-message:0", 1_200.0);
        assert_eq!(first_reopen, 1_200.0);
        assert_eq!(offsets.get("tool-message:0"), Some(&1_200.0));

        let second_reopen = diff_scroll_offset(&mut offsets, "tool-message:0", 300.0);
        assert_eq!(second_reopen, 300.0);
        assert_eq!(offsets.get("tool-message:0"), Some(&300.0));

        let final_reopen = diff_scroll_offset(&mut offsets, "tool-message:0", 0.0);
        assert_eq!(final_reopen, 0.0);
        assert!(!offsets.contains_key("tool-message:0"));
    }

    #[test]
    fn update_hover_target_only_reports_real_changes() {
        let mut hover = None;
        assert!(update_hover_target(&mut hover, Some("a".to_string())));
        assert!(!update_hover_target(&mut hover, Some("a".to_string())));
        assert!(update_hover_target(&mut hover, Some("b".to_string())));
        assert!(update_hover_target(&mut hover, None));
        assert!(!update_hover_target(&mut hover, None));
    }
}
