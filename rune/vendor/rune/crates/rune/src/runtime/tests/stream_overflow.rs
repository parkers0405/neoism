use super::*;
use crate::el::GrowthSpec;
use crate::growth::{StreamEdge, edge};

/// A stack whose streaming row grows from 40 to 400 pt and whose fold shuts from 600 to 40 pt,
/// under `edge`.
fn masks(edge: StreamEdge) -> (Vec<f32>, Vec<f32>) {
    fn view(s: &(bool, StreamEdge), _: &mut Cx<(bool, StreamEdge)>) -> El {
        let reply = el().key("reply").h(if s.0 { 400.0 } else { 40.0 }).w_full().streaming(true);
        let fold = el().key("fold").h(if s.0 { 40.0 } else { 600.0 }).w_full().fold_glide(rune_motion::Spring::new(0.18, 0.0)).fold_feather(16.0);
        col().key("root").child(col().key("stack").growth(GrowthSpec { edge: s.1, ..GrowthSpec::default() }).children([fold, reply]))
    }
    let mut rt = Runtime::new((false, edge), view, (400.0, 1400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|s, _| s.0 = true);
    let at = |rt: &Runtime<(bool, StreamEdge)>, k: &str| rt.nodes.iter().position(|n| n.named(k)).unwrap();
    (0..60).map(|_| {
        rt.frame(1.0 / 120.0);
        (rt.row_mask[at(&rt, "reply")], rt.row_mask[at(&rt, "fold")])
    }).unzip()
}

#[test]
fn an_overflowing_edge_leaves_only_streaming_rows_unclipped() {
    let (reply, fold) = masks(edge::FEATHER);
    assert!(reply.iter().any(|&m| m >= 0.0), "the feather clips a growing reply: {reply:?}");
    let (reply, fold_over) = masks(StreamEdge { overflow: true, ..edge::FEATHER });
    assert!(reply.iter().all(|&m| m < 0.0), "overflowing, the reply draws past its height: {reply:?}");
    assert_eq!(fold, fold_over, "a shutting fold clips as before");
    assert!(fold_over.iter().any(|&m| m >= 0.0), "and does clip: {fold_over:?}");
}
