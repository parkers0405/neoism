use super::*;

#[test]
fn agent_model_thinking_rects_are_pointer_targets_not_text_input() {
    let mut hits = ComposerInteractions::default();
    for index in 0..3 {
        hits.register_status_chip_rect(index, [10.0 + index as f32 * 90.0, 100.0, 80.0, 24.0]);
    }
    for index in 0..3 {
        let x = 20.0 + index as f32 * 90.0;
        assert_eq!(hits.status_chip_at(x, 110.0), Some(index));
        assert!(hits.pointer_at(x, 110.0));
        // Chips are not folder/attachment commands.
        assert_eq!(hits.action_at(x, 110.0), None);
    }
    assert!(!hits.pointer_at(20.0, 60.0)); // text composer
    assert!(!hits.pointer_at(95.0, 110.0)); // inter-chip gap
    assert!(!hits.pointer_at(90.0, 110.0)); // half-open right edge
}

#[test]
fn hidden_and_covered_picker_chips_are_not_clickable_or_pointer_targets() {
    let mut hits = ComposerInteractions::default();
    hits.register_status_chip_rect(1, [10.0, 100.0, 80.0, 24.0]);
    hits.occlusion_rects.push([40.0, 105.0, 100.0, 50.0]);
    assert!(!hits.pointer_at(50.0, 110.0));
    assert_eq!(hits.status_chip_at(50.0, 110.0), None);
    assert!(hits.pointer_at(20.0, 110.0));
    assert!(hits.pointer_at(50.0, 102.0)); // visible above overlay
    hits.occlusion_rects.push([10.0, 100.0, 80.0, 24.0]);
    assert!(!hits.pointer_at(20.0, 110.0));
    hits.clear_rects();
    assert_eq!(hits.status_chip_at(20.0, 110.0), None);
    assert!(!hits.pointer_at(20.0, 110.0));
    hits.register_status_chip_rect(2, [10.0, 100.0, 0.0, 24.0]);
    assert!(!hits.pointer_at(10.0, 110.0));
}

#[test]
fn cwd_hit_excludes_branch_trailing_gap_and_hint_area() {
    let hits = cwd_visible_hit_rects([10.0, 100.0, 500.0, 24.0], 83.0, 200.0, &[]);
    assert_eq!(hits, vec![[10.0, 100.0, 83.0, 24.0]]);
    let mut state = ComposerInteractions::default();
    state.cwd_rects = hits;
    assert_eq!(state.action_at(92.0, 110.0), Some(ComposerAction::ChangeDirectory));
    assert_eq!(state.action_at(93.0, 110.0), None);
    assert_eq!(state.action_at(120.0, 110.0), None);
    assert_eq!(cwd_visible_hit_rects([10.0, 100.0, 500.0, 24.0], 200.0, 31.0, &[]), vec![[10.0, 100.0, 31.0, 24.0]]);
}

#[test]
fn hidden_and_occluded_cwd_has_no_invisible_hit_target() {
    let rect = [10.0, 100.0, 200.0, 24.0];
    assert!(cwd_visible_hit_rects(rect, 100.0, 0.0, &[]).is_empty());
    assert!(cwd_visible_hit_rects(rect, 100.0, 200.0, &[rect]).is_empty());
    let hits = cwd_visible_hit_rects(rect, 100.0, 200.0, &[[40.0, 90.0, 30.0, 50.0]]);
    assert_eq!(hits, vec![[10.0, 100.0, 30.0, 24.0], [70.0, 100.0, 40.0, 24.0]]);
    let mut state = ComposerInteractions::default();
    state.cwd_rects = hits;
    assert_eq!(state.action_at(50.0, 110.0), None);
    assert_eq!(state.action_at(75.0, 110.0), Some(ComposerAction::ChangeDirectory));
    state.clear_rects();
    assert_eq!(state.action_at(75.0, 110.0), None);
}

#[test]
fn layout_invalidation_removes_all_footer_targets() {
    let mut state = ComposerInteractions::default();
    state.cwd_rect = Some([0.0, 0.0, 50.0, 20.0]);
    state.cwd_rects.push([0.0, 30.0, 50.0, 20.0]);
    state.clear_rects();
    assert_eq!(state.action_at(1.0, 1.0), None);
    assert_eq!(state.action_at(1.0, 31.0), None);
}
