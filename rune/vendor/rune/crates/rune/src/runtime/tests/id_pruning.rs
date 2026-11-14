use super::*;
use crate::el::GrowthSpec;

fn view(start: &usize, _: &mut Cx<usize>) -> El {
    let stack = col().key("stack").w_full().growth(GrowthSpec::default())
        .children((*start..*start + 3).map(|i| el().key(format!("r{i}")).w_full().child(el().key("label").h(20.0))));
    col().key("root").size(200.0, 200.0).child(stack)
}

#[test]
fn rows_that_left_leave_no_ids_or_stack_tops_behind() {
    let mut rt = Runtime::new(0usize, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let mounted = rt.path_ids.len();
    for _ in 0..40 {
        rt.update(|n, _| *n += 3);
        for _ in 0..90 { rt.frame(1.0 / 60.0); }
    }
    let stack = rt.nodes.iter().find(|n| n.key.as_deref() == Some("stack")).unwrap().id;
    let tops = rt.stacks[&stack].tops.len();
    assert!(tops <= 6, "{tops} row tops kept for 3 rows");
    assert!(rt.path_ids.len() <= mounted + 6, "{} ids kept for {mounted} mounted paths", rt.path_ids.len());
    assert!(rt.find("r120").is_some(), "the current rows still draw");
}

#[test]
fn a_path_keeps_its_id_while_it_holds_state() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        let list = col().children((0..20).map(|i| el().key(format!("i{i}")).h(40.0)));
        let scroll = col().key("scroll").size(100.0, 100.0).scroll(ScrollSpec { stick_bottom: false }).child(list);
        col().key("root").size(100.0, 100.0).children(on.then_some(scroll))
    }
    let mut rt = Runtime::new(true, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.wheel(50.0, 50.0, -120.0);
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("scroll")).unwrap().id;
    rt.update(|on, tx| { *on = false; tx.instant(); });
    rt.frame(1.0 / 60.0);
    rt.update(|on, tx| { *on = true; tx.instant(); });
    rt.frame(1.0 / 60.0);
    let again = rt.nodes.iter().find(|n| n.key.as_deref() == Some("scroll")).unwrap().id;
    assert_eq!(id, again, "a scroll view coming back finds its scroll state");
}

/// Commits and prunes trade two node buffers instead of growing a new one each time a row
/// leaves.
#[test]
fn pruning_left_rows_reuses_the_node_buffers() {
    let mut rt = Runtime::new(0usize, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let mut buffers = std::collections::HashSet::new();
    for round in 0..12 {
        rt.update(|n, _| *n += 3);
        for _ in 0..90 { rt.frame(1.0 / 60.0); }
        if round >= 4 {
            buffers.insert(rt.nodes.as_ptr() as usize);
            assert!(rt.spare_nodes.capacity() >= rt.nodes.len(), "the spare holds a whole commit");
        }
    }
    assert!(buffers.len() <= 2, "{} node buffers in 8 rounds", buffers.len());
}
