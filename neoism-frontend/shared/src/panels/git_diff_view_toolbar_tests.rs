#[cfg(test)]
pub(super) fn test_mode_targets() {
    let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
    view.toolbar_hits = vec![
        Hit {
            rect: Rect::new(0.0, 0.0, 26.0, 36.0),
            control: Control::Unified,
            enabled: true,
        },
        Hit {
            rect: Rect::new(26.0, 0.0, 26.0, 36.0),
            control: Control::Split,
            enabled: true,
        },
    ];
    assert!(view.pointer_down(10.0, 10.0));
    assert!(!view.split);
    assert!(view.pointer_down(10.0, 10.0));
    assert!(!view.split);
    assert!(view.pointer_down(30.0, 10.0));
    assert!(view.split);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, staged: bool, status: FileStatus) -> FileChange {
        FileChange {
            path: path.into(),
            staged,
            status,
            additions: 12,
            deletions: 9,
        }
    }

    #[test]
    fn measured_groups_never_clip_or_overlap_at_any_width_or_scale() {
        for scale in [0.7, 1.0, 1.25, 1.5, 2.5] {
            // Non-integral, deliberately wide measured text (large totals).
            let widths =
                [26.0, 52.0, 175.25, 52.0, 61.5, 75.25, 59.75].map(|w| w * scale);
            for width in 0..1400 {
                let rect = Rect::new(31.5, 17.0, width as f32, 36.0 * scale);
                let groups = layout(rect, scale, widths);
                for g in &groups {
                    assert!(g.rect.x >= rect.x);
                    assert!(g.rect.x + g.rect.w <= rect.x + rect.w + 0.001);
                    let index = match g.group {
                        Group::Fold => 0,
                        Group::Modes => 1,
                        Group::Totals => 2,
                        Group::Navigation => 3,
                        Group::Stage => 4,
                        Group::StageAll => 5,
                        Group::Commit => 6,
                    };
                    assert_eq!(g.rect.w, widths[index]);
                }
                for pair in groups.windows(2) {
                    assert!(pair[0].rect.x + pair[0].rect.w <= pair[1].rect.x);
                }
                let modes = groups.iter().find(|g| g.group == Group::Modes);
                if let Some(modes) = modes {
                    assert_eq!(modes.rect.w, 52.0 * scale);
                }
            }
            let full = layout(
                Rect::new(0.0, 0.0, 2000.0 * scale, 36.0 * scale),
                scale,
                widths,
            );
            assert_eq!(full.len(), 7);
            assert!(
                (full.last().unwrap().rect.x + full.last().unwrap().rect.w
                    - 1992.0 * scale)
                    .abs()
                    < 0.001 * scale
            );
        }
    }

    #[test]
    fn no_tab_hint_paint_target_or_reserved_width() {
        let source = include_str!("git_diff_view_toolbar.rs");
        assert!(!source.contains("[Tab]"));
        assert!(!source.contains("Group::Hint"));
        // Exact fit includes only seven real groups and six gaps: no hint slot.
        let widths = [26.0, 52.0, 40.0, 52.0, 50.0, 70.0, 60.0];
        let width = widths.iter().sum::<f32>() + 6.0 * 8.0 + 16.0;
        let groups = layout(Rect::new(0.0, 0.0, width, 36.0), 1.0, widths);
        assert_eq!(groups.len(), 7);
        assert_eq!(groups[2].rect.x, groups[1].rect.x + groups[1].rect.w + 8.0);
        // Neither a keyboard-hint hit target nor any leftover gap consumes clicks.
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        assert!(
            !view.toolbar_pointer_down(groups[1].rect.x + groups[1].rect.w + 4.0, 18.0)
        );
        let split = view.split;
        assert!(view.key("Tab"));
        assert_ne!(view.split, split);
        assert!(view.key("Tab"));
        assert_eq!(view.split, split);
    }

    #[test]
    fn structural_icons_have_visible_bounded_geometry_at_every_scale() {
        assert!(
            ICON_ORDER > 20,
            "icons must paint above the review background"
        );
        for scale in [0.7, 1.0, 1.25, 2.5] {
            let rect = Rect::new(31.5, 17.0, 26.0 * scale, 28.0 * scale);
            for (control, count) in [
                (Control::Fold, 8),
                (Control::Unified, 12),
                (Control::Split, 10),
                (Control::Previous, 6),
                (Control::Next, 6),
            ] {
                for collapsed in [false, true] {
                    let triangles = icon_triangles(control, collapsed, rect, scale);
                    assert_eq!(triangles.len(), count);
                    for [a, b, c] in triangles {
                        for (x, y) in [a, b, c] {
                            assert!(x.is_finite() && y.is_finite());
                            assert!(x > rect.x && x < rect.x + rect.w);
                            assert!(y > rect.y && y < rect.y + rect.h);
                        }
                        let area =
                            ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).abs();
                        assert!(area > 0.1 * scale * scale);
                    }
                }
            }
            assert_ne!(
                icon_triangles(Control::Fold, true, rect, scale),
                icon_triangles(Control::Fold, false, rect, scale)
            );
            assert_ne!(
                icon_triangles(Control::Previous, false, rect, scale),
                icon_triangles(Control::Next, false, rect, scale)
            );
        }
    }

    #[test]
    fn selected_file_queues_explicit_directions_and_root_scoped_bulk_actions() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        for (staged, status, unstage) in [
            (false, FileStatus::Modified, false),
            (true, FileStatus::Modified, true),
            (true, FileStatus::Mixed, false),
        ] {
            view.set_files(vec![file("a.rs", staged, status)]);
            view.toolbar_activate(Control::Stage);
            assert_eq!(
                view.take_actions(),
                vec![GitDiffViewAction::Stage {
                    root: "/repo".into(),
                    path: "a.rs".into(),
                    unstage
                }]
            );
        }
        view.toolbar_activate(Control::StageAll);
        view.toolbar_activate(Control::Commit);
        assert_eq!(
            view.take_actions(),
            vec![
                GitDiffViewAction::StageAll {
                    root: "/repo".into()
                },
                GitDiffViewAction::Commit {
                    root: "/repo".into()
                }
            ]
        );
        view.set_actions_loading(true);
        for c in [Control::Stage, Control::StageAll, Control::Commit] {
            view.toolbar_activate(c);
        }
        assert!(view.take_actions().is_empty());
        view.set_actions_loading(false);
        view.set_files(vec![file("a.rs", true, FileStatus::Modified)]);
        assert!(!view.toolbar_enabled(Control::StageAll));
    }

    #[test]
    fn fold_all_and_navigation_reveal_the_relevant_section() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.set_files(vec![
            file("a.rs", false, FileStatus::Modified),
            file("b.rs", true, FileStatus::Modified),
        ]);
        assert!(!view.toolbar_enabled(Control::Previous));
        view.toolbar_activate(Control::Fold);
        assert!(view.sections.iter().all(|s| s.collapsed));
        view.toolbar_activate(Control::Next);
        assert_eq!(view.path, PathBuf::from("b.rs"));
        assert_eq!(view.pending_reveal, Some(PathBuf::from("b.rs")));
        assert!(!view.sections[1].collapsed);
        assert!(!view.toolbar_enabled(Control::Next));
        view.toolbar_activate(Control::Previous);
        assert_eq!(view.path, PathBuf::from("a.rs"));
        assert!(!view.sections[0].collapsed);
        view.toolbar_activate(Control::Fold);
        view.toolbar_activate(Control::Fold);
        assert!(view.sections.iter().all(|s| !s.collapsed));
    }

    #[test]
    fn empty_state_and_stale_disabled_hits_cannot_enqueue_mutations() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.set_files(Vec::new());
        for control in [
            Control::Fold,
            Control::Previous,
            Control::Next,
            Control::Stage,
            Control::StageAll,
            Control::Commit,
        ] {
            assert!(!view.toolbar_enabled(control));
            view.toolbar_hits = vec![Hit {
                rect: Rect::new(0.0, 0.0, 60.0, 36.0),
                control,
                enabled: true,
            }];
            assert!(view.pointer_down(10.0, 10.0));
        }
        assert!(view.take_actions().is_empty());
        view.set_files(vec![file("a.rs", false, FileStatus::Modified)]);
        view.toolbar_hits = vec![Hit {
            rect: Rect::new(0.0, 0.0, 60.0, 36.0),
            control: Control::Stage,
            enabled: false,
        }];
        assert!(view.pointer_down(10.0, 10.0));
        assert!(view.take_actions().is_empty());
        view.set_text_occlusions(&[[0.0, 0.0, 60.0, 36.0]]);
        assert!(!view.pointer_down(10.0, 10.0));
    }
}
