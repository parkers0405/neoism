//! Stack-overflow regressions run in a subprocess so a failure cannot abort the
//! main test runner. The actual layout executes on a deliberately small stack.
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use neoui::{Element, LayoutError, Rect, Runtime, TextStyle};

const CHILD: &str = "NEOUI_DEEP_TREE_CLEANUP_CHILD";
const DEPTH: usize = 50_000;

#[derive(Clone, Debug)]
struct DropCount(Arc<AtomicUsize>);
impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn deep_tree(dropped: &Arc<AtomicUsize>) -> Element<DropCount> {
    let mut root = Element::spacer("leaf").on_pointer_down(DropCount(dropped.clone()));
    for index in 0..DEPTH {
        root = Element::column(format!("node/{index}"), [root])
            .on_pointer_down(DropCount(dropped.clone()));
    }
    root
}

#[test]
fn rejected_deep_trees_are_disposed_on_two_mib_stack() {
    let result = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "deep_tree_cleanup_worker", "--nocapture"])
        .env(CHILD, "1")
        .output()
        .expect("start isolated stack regression");
    assert!(
        result.status.success(),
        "deep-tree cleanup subprocess failed: {}\n{}\n{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn deep_tree_cleanup_worker() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    std::thread::Builder::new()
        .name("neoui-two-mib-cleanup".into())
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            for case in 0..8 {
                let dropped = Arc::new(AtomicUsize::new(0));
                let mut root = deep_tree(&dropped);
                let mut expected_drops = DEPTH + 1;
                let mut origin = [0.0, 0.0];
                let mut width = 100.0;
                let mut clip = Rect::new(0.0, 0.0, 100.0, 100.0);
                match case {
                    1 => origin[0] = f32::NAN,
                    2 => width = -1.0,
                    3 => clip.width = -1.0,
                    4 => {
                        root = Element::column("", [root]);
                    }
                    5 => {
                        root = Element::column(format!("node/{}", DEPTH - 1), [root]);
                    }
                    6 => root = root.gap(f32::NAN),
                    7 => {
                        // Validation exits before ever visiting the deep tail.
                        // Every remaining sibling must still be disposed safely.
                        let mut siblings: Vec<_> = (0..100_000)
                            .map(|i| {
                                Element::spacer(format!("sibling/{i}"))
                                    .on_pointer_down(DropCount(dropped.clone()))
                            })
                            .collect();
                        expected_drops += siblings.len();
                        siblings.push(root);
                        root = Element::column("wide-root", siblings);
                    }
                    _ => {}
                }
                let mut runtime = Runtime::new();
                let mut measure = |_: &str, _: &TextStyle| 0.0;
                let error = runtime
                    .layout(root, origin, width, clip, &mut measure)
                    .unwrap_err();
                match case {
                    0 => assert_eq!(error, LayoutError::TreeTooDeep),
                    1..=3 | 6 => {
                        assert!(matches!(error, LayoutError::InvalidInput { .. }))
                    }
                    4 => assert_eq!(error, LayoutError::EmptyKey),
                    5 => assert!(matches!(error, LayoutError::DuplicateKey(_))),
                    7 => assert_eq!(error, LayoutError::TooManyElements),
                    _ => unreachable!(),
                }
                // This also verifies that safe disposal did not leak descendants
                // to avoid recursion. Every action is destroyed exactly once.
                assert_eq!(
                    dropped.load(Ordering::Relaxed),
                    expected_drops,
                    "case {case}"
                );
                assert!(runtime.frame().hits.is_empty());
            }
        })
        .expect("spawn small-stack thread")
        .join()
        .expect("small-stack cleanup worker");
}
