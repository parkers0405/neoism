//! GPUIX adapter for the canonical desktop session tree and geometry solver.
use neoism_ui::{
    layout::Rect,
    session_layout::{
        geometry::{self, DropPlacement, SolveOpts},
        nearest_horizontal_pane, nearest_vertical_pane,
        tree::{SessionTree, SessionTreeLeafId, SessionTreeNode, VisualDir},
        SessionLeafKind, SessionLeafSpec, SessionPaneRect, SplitAxis, SplitPlacement,
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashMap};

#[derive(Default)]
struct HostLayout {
    tree: Option<SessionTree>,
    width: f32,
    height: f32,
    scale: f32,
}
#[derive(Default)]
struct Store {
    next: u32,
    layouts: HashMap<u32, HostLayout>,
}
thread_local! { static STORE: RefCell<Store> = RefCell::new(Store::default()); }
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn number(v: &Value, key: &str, default: f32) -> f32 {
    v[key]
        .as_f64()
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .unwrap_or(default)
}
fn leaf(v: &Value, key: &str) -> Result<SessionTreeLeafId, String> {
    v[key]
        .as_u64()
        .filter(|n| *n > 0)
        .map(SessionTreeLeafId)
        .ok_or_else(|| format!("Missing {key}"))
}
fn axis(v: &Value) -> SplitAxis {
    if text(v, "axis") == "vertical" {
        SplitAxis::Vertical
    } else {
        SplitAxis::Horizontal
    }
}
fn kind(value: &str) -> SessionLeafKind {
    match value {
        "file" => SessionLeafKind::Editor,
        "agent" => SessionLeafKind::Agent,
        "page" | "welcome" => SessionLeafKind::Editor,
        "git" => SessionLeafKind::Custom("git".into()),
        _ => SessionLeafKind::Terminal,
    }
}
fn kind_name(value: &SessionLeafKind) -> &'static str {
    match value {
        SessionLeafKind::Editor => "file",
        SessionLeafKind::Agent => "agent",
        SessionLeafKind::Custom(name) if name == "git" => "git",
        _ => "terminal",
    }
}
fn spec(v: &Value) -> Result<SessionLeafSpec, String> {
    let external = v["externalId"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("Missing externalId")?;
    Ok(SessionLeafSpec::new(kind(text(v, "kind")))
        .with_external_id(external)
        .with_title(text(v, "title")))
}
fn placement(v: &Value) -> SplitPlacement {
    if v["before"].as_bool().unwrap_or(false) {
        SplitPlacement::Before
    } else {
        SplitPlacement::After
    }
}
fn rect(r: Rect) -> [f32; 4] {
    [r.x, r.y, r.w, r.h]
}
fn err<E: std::fmt::Debug>(error: E) -> String {
    format!("{error:?}")
}
fn first_leaf(node: &SessionTreeNode) -> Option<SessionTreeLeafId> {
    match node {
        SessionTreeNode::Leaf(leaf) => Some(leaf.id),
        SessionTreeNode::Split { children, .. } => children.iter().find_map(first_leaf),
        SessionTreeNode::Tabbed { children, active } => {
            children.get(*active).and_then(first_leaf)
        }
    }
}
fn replace_node(
    root: &mut SessionTreeNode,
    path: &[usize],
    replacement: SessionTreeNode,
) -> Result<(), String> {
    if path.is_empty() {
        *root = replacement;
        return Ok(());
    }
    match root {
        SessionTreeNode::Split { children, .. }
        | SessionTreeNode::Tabbed { children, .. } => replace_node(
            children.get_mut(path[0]).ok_or("Invalid pane group")?,
            &path[1..],
            replacement,
        ),
        SessionTreeNode::Leaf(_) => Err("Invalid pane group".into()),
    }
}
/// A host pane owns a local tab group. Split that complete group while using
/// the canonical operation to allocate the sibling and decide axis/ratios.
fn split_pane(
    tree: &mut SessionTree,
    axis: SplitAxis,
    placement: SplitPlacement,
    spec: SessionLeafSpec,
) -> Result<SessionTreeLeafId, String> {
    let focus = tree.focus();
    let path = tree.path_to_leaf(focus).ok_or("Missing focused pane")?;
    let group = path
        .split_last()
        .and_then(|(_, parent)| match tree.node_at(parent) {
            Some(node @ SessionTreeNode::Tabbed { children, .. })
                if children
                    .iter()
                    .all(|c| matches!(c, SessionTreeNode::Leaf(_))) =>
            {
                Some((parent.to_vec(), node.clone()))
            }
            _ => None,
        });
    let outcome = tree.split_focused(axis, placement, spec).map_err(err)?;
    if let Some((group_path, old_group)) = group {
        let mut split = tree
            .node_at(&outcome.split_path)
            .cloned()
            .ok_or("Missing new split")?;
        if let SessionTreeNode::Split { children, .. } = &mut split {
            let old_index = if placement == SplitPlacement::Before {
                1
            } else {
                0
            };
            children[old_index] = old_group;
        }
        let mut root = tree.root().clone();
        replace_node(&mut root, &group_path, split)?;
        *tree = SessionTree::from_root(root, outcome.new_leaf).map_err(err)?;
    }
    Ok(outcome.new_leaf)
}
fn tabs(tree: &SessionTree, id: SessionTreeLeafId) -> Value {
    let path = tree.path_to_leaf(id).unwrap_or_default();
    for depth in (0..path.len()).rev() {
        if let Some(SessionTreeNode::Tabbed { children, .. }) =
            tree.node_at(&path[..depth])
        {
            return Value::Array(children.iter().filter_map(first_leaf).filter_map(|id| tree.leaf(id))
                .map(|leaf| json!({"leaf":leaf.id.0,"externalId":leaf.external_id,"title":leaf.title,"kind":kind_name(&leaf.kind)})).collect());
        }
    }
    tree.leaf(id).map(|leaf| json!([{"leaf":leaf.id.0,"externalId":leaf.external_id,"title":leaf.title,"kind":kind_name(&leaf.kind)}])).unwrap_or(json!([]))
}
fn subtree_rect(
    tree: &SessionTree,
    path: &[usize],
    content: Rect,
    opts: &SolveOpts,
) -> Rect {
    let mut result = content;
    let mut node = tree.root();
    for &index in path {
        match node {
            SessionTreeNode::Tabbed { children, .. } => {
                if let Some(child) = children.get(index) {
                    node = child;
                } else {
                    break;
                }
            }
            SessionTreeNode::Split {
                axis,
                children,
                ratios,
            } => {
                let Some(child) = children.get(index) else {
                    break;
                };
                let horizontal = *axis == SplitAxis::Horizontal;
                let gap = if horizontal { opts.gap_x } else { opts.gap_y };
                let available = ((if horizontal { result.w } else { result.h })
                    - gap * children.len().saturating_sub(1) as f32)
                    .max(0.0);
                let shares = geometry::ratios_to_shares(ratios, children.len());
                let offset = available * shares.iter().take(index).sum::<f32>()
                    + index as f32 * gap;
                let length = available * shares.get(index).copied().unwrap_or(0.0);
                result = if horizontal {
                    Rect::new(result.x + offset, result.y, length, result.h)
                } else {
                    Rect::new(result.x, result.y + offset, result.w, length)
                };
                node = child;
            }
            SessionTreeNode::Leaf(_) => break,
        }
    }
    result
}
impl HostLayout {
    fn opts(&self) -> SolveOpts {
        SolveOpts {
            gap_x: 4.0 * self.scale,
            gap_y: 4.0 * self.scale,
            margin: 0.0,
            divider_tol: 3.0 * self.scale,
        }
    }
    fn content(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width, self.height)
    }
    fn frame(&self) -> Value {
        let Some(tree) = &self.tree else {
            return json!({"focusedLeaf":null,"focusedExternalId":null,"panes":[],"dividers":[],"leaves":[],"tree":null});
        };
        let opts = self.opts();
        let solved = geometry::solve_with(tree, self.content(), &opts);
        let panes:Vec<Value> = solved.panes.iter().map(|p| json!({"leaf":p.leaf.0,"externalId":p.external_id,"rect":rect(p.rect),"focused":p.focused,"path":p.path,"tabs":tabs(tree,p.leaf)})).collect();
        let dividers:Vec<Value> = solved.dividers.iter().map(|d| {
            let split_rect = subtree_rect(tree,&d.split_path,self.content(),&opts);
            let count = match tree.node_at(&d.split_path) { Some(SessionTreeNode::Split { children, .. }) => children.len(), _ => 1 };
            let available = ((if d.axis==SplitAxis::Horizontal {split_rect.w} else {split_rect.h}) - 4.0*self.scale*count.saturating_sub(1) as f32).max(1.0);
            let ratio = match tree.node_at(&d.split_path) { Some(SessionTreeNode::Split{ratios,..}) => ratios.get(d.gap).copied().unwrap_or(0.5), _ => 0.5 };
            json!({"path":d.split_path,"gap":d.gap,"axis":if d.axis==SplitAxis::Horizontal{"horizontal"}else{"vertical"},"rect":rect(d.rect),"ratio":ratio,"splitRect":rect(split_rect),"available":available})
        }).collect();
        let leaves:Vec<Value> = tree.all_leaves().iter().filter_map(|id|tree.leaf(*id)).map(|l|json!({"leaf":l.id.0,"externalId":l.external_id,"title":l.title,"kind":kind_name(&l.kind)})).collect();
        json!({"focusedLeaf":tree.focus().0,"focusedExternalId":tree.leaf(tree.focus()).and_then(|l|l.external_id),"panes":panes,"dividers":dividers,"leaves":leaves,"tree":tree})
    }
}
pub fn dispatch(v: &Value) -> Option<Result<Value, String>> {
    let op = text(v, "op");
    if !op.starts_with("layout.") {
        return None;
    }
    if op == "layout.chrome" {
        use neoism_ui::panels::buffer_tabs::{consts, BufferTabs, BUFFER_TABS_HEIGHT};
        let scale = number(v, "scale", 1.0).clamp(0.5, 3.0);
        let geometry = BufferTabs::<()>::visual_tab_geometry(number(v, "titleWidth", 0.0), number(v, "iconWidth", consts::ICON_FONT_SIZE * scale), v["close"].as_bool().unwrap_or(true), scale);
        let workspace = neoism_ui::widgets::island::island_chrome_spec(scale);
        return Some(Ok(json!({"workspace":workspace,"buffer":{"height":BUFFER_TABS_HEIGHT*scale,"fontSize":consts::FONT_SIZE*scale,"iconSize":consts::ICON_FONT_SIZE*scale,"iconGap":consts::ICON_GAP*scale,"padding":consts::TAB_PADDING_X*scale,"closeSize":consts::CLOSE_BTN_SIZE*scale,"closeHitSize":consts::CLOSE_HIT_SIZE*scale,"newWidth":consts::NEW_TAB_BTN_WIDTH*scale,"newIcon":consts::NEW_TAB_ICON,"width":geometry.tab_width,"titleWidth":geometry.title_clip_width,"iconWidth":geometry.icon_width,"closeCenter":geometry.close_center_x}})));
    }
    Some(STORE.with(|store| {
        let mut store = store.borrow_mut();
        if op == "layout.new" {
            store.next += 1; let id = 0x50000000 + store.next;
            let mut layout = HostLayout { scale: 1.0, ..HostLayout::default() };
            if !v["tree"].is_null() { let tree: SessionTree = serde_json::from_value(v["tree"].clone()).map_err(err)?; tree.validate().map_err(err)?; layout.tree=Some(tree); }
            let frame=layout.frame(); store.layouts.insert(id,layout); return Ok(json!({"id":id,"frame":frame}));
        }
        let id = v["id"].as_u64().unwrap_or(0) as u32;
        if op=="layout.dispose" { store.layouts.remove(&id); return Ok(json!({})); }
        let layout=store.layouts.get_mut(&id).ok_or("Unknown layout")?;
        if op=="layout.restore" {
            let tree = if v["tree"].is_null() {None} else {let tree:SessionTree=serde_json::from_value(v["tree"].clone()).map_err(err)?; tree.validate().map_err(err)?; Some(tree)};
            layout.tree=tree; return Ok(layout.frame());
        }
        if op=="layout.restore-daemon" {
            let snapshot = serde_json::from_value(v["snapshot"].clone()).map_err(err)?;
            layout.tree=Some(SessionTree::from_pane_layout_snapshot(&snapshot).map_err(err)?); return Ok(layout.frame());
        }
        if op=="layout.size" {layout.width=number(v,"width",layout.width).max(0.0);layout.height=number(v,"height",layout.height).max(0.0);layout.scale=number(v,"scale",1.0).clamp(0.5,3.0);return Ok(layout.frame());}
        if op=="layout.open" || op=="layout.split" {
            let spec=spec(v)?;
            if let Some(tree)=layout.tree.as_mut() {
                if op=="layout.open" {
                    if let Some(existing)=tree.all_leaves().into_iter().find(|id|tree.leaf(*id).and_then(|l|l.external_id)==spec.external_id) {tree.focus_leaf(existing).map_err(err)?;}
                    else {tree.wrap_leaf_in_tabbed(tree.focus(),spec).map_err(err)?;}
                } else {split_pane(tree,axis(v),placement(v),spec)?;}
            } else {layout.tree=Some(SessionTree::new(spec));}
            return Ok(layout.frame());
        }
        if op=="layout.frame" || op=="layout.snapshot" {return Ok(layout.frame());}
        let content=layout.content(); let opts=layout.opts();
        let Some(tree)=layout.tree.as_mut() else{return Ok(layout.frame());};
        match op {
            "layout.first-tab" => {
                let target=leaf(v,"leaf")?;
                if let Some(path)=tree.path_to_leaf(target){if let Some((index,parent))=path.split_last(){if matches!(tree.node_at(parent),Some(SessionTreeNode::Tabbed{..})){tree.move_tab(parent,*index,0).map_err(err)?;}}}
            }
            "layout.reorder" => {
                let source=leaf(v,"leaf")?;let target=leaf(v,"target")?;
                let source_path=tree.path_to_leaf(source).ok_or("Unknown source tab")?;
                let target_path=tree.path_to_leaf(target).ok_or("Unknown target tab")?;
                if source!=target {
                    let (from,parent)=source_path.split_last().ok_or("Source is not a tab group")?;
                    let (target_index,target_parent)=target_path.split_last().ok_or("Target is not a tab group")?;
                    if parent!=target_parent||!matches!(tree.node_at(parent),Some(SessionTreeNode::Tabbed{..})){return Err("Tabs belong to different pane groups".into());}
                    let insertion=*target_index+usize::from(!v["before"].as_bool().unwrap_or(true));
                    let to=insertion-usize::from(*from<insertion);
                    tree.move_tab(parent,*from,to).map_err(err)?;
                }
            }
            "layout.focus" => {tree.focus_leaf(leaf(v,"leaf")?).map_err(err)?;}
            "layout.rename" => {let l=tree.leaf_mut(leaf(v,"leaf")?).ok_or("Unknown leaf")?;l.title=Some(text(v,"title").to_owned());}
            "layout.close" => {
                let target=leaf(v,"leaf")?;
                if tree.all_leaves().len()==1 {if tree.leaf(target).is_none(){return Err("Unknown leaf".into());}layout.tree=None;}
                else {tree.detach_leaf(target).map_err(err)?;}
            }
            "layout.resize" => {
                let path:Vec<usize>=serde_json::from_value(v["path"].clone()).map_err(err)?;
                tree.set_ratio(&path,v["gap"].as_u64().unwrap_or(0)as usize,number(v,"ratio",0.5)).map_err(err)?;
            }
            "layout.nudge" => {tree.resize_event(Some(axis(v)),number(v,"delta",0.05)).map_err(err)?;}
            "layout.focus-direction" => {
                let direction=text(v,"direction");
                if ["next","previous","first","last"].contains(&direction) {
                    tree.focus_next_visual(match direction {"previous"=>VisualDir::Previous,"first"=>VisualDir::First,"last"=>VisualDir::Last,_=>VisualDir::Next}).map_err(err)?;
                } else {
                    let solved=geometry::solve_with(tree,content,&opts);
                    if let Some(current)=solved.panes.iter().find(|p|p.focused) {
                        let current=SessionPaneRect::new(current.leaf,rect(current.rect));
                        let panes=solved.panes.iter().map(|p|SessionPaneRect::new(p.leaf,rect(p.rect)));
                        let next=if direction=="left"||direction=="right"{nearest_horizontal_pane(current,panes,direction=="right")}else{nearest_vertical_pane(current,panes,direction=="down")};
                        if let Some(next)=next{tree.focus_leaf(next).map_err(err)?;}
                    }
                }
            }
            "layout.drop-preview" | "layout.drop" => {
                let solved=geometry::solve_with(tree,content,&opts);
                let zone=geometry::drop_zone_at(&solved,number(v,"x",0.0),number(v,"y",0.0),0.25);
                if op=="layout.drop-preview" {return Ok(json!({"target":zone.as_ref().map(|z|z.target.0),"placement":zone.as_ref().map(|z|format!("{:?}",z.placement).to_lowercase()),"rect":zone.map(|z|rect(z.highlight))}));}
                if let Some(zone)=zone {
                    let source=leaf(v,"leaf")?;
                    if source!=zone.target {
                        // Mutate a clone so a rejected target never drops the source.
                        let mut moved=tree.clone();let old=moved.detach_leaf(source).map_err(err)?;
                        moved.focus_leaf(zone.target).map_err(err)?;
                        if zone.placement==DropPlacement::Center {moved.insert_leaf_as_tab_sibling(zone.target,old).map_err(err)?;}
                        else {
                            let axis=if matches!(zone.placement,DropPlacement::Left|DropPlacement::Right){SplitAxis::Horizontal}else{SplitAxis::Vertical};
                            let placement=if matches!(zone.placement,DropPlacement::Left|DropPlacement::Top){SplitPlacement::Before}else{SplitPlacement::After};
                            let spec=SessionLeafSpec{kind:old.kind,title:old.title,external_id:old.external_id};
                            let new=split_pane(&mut moved,axis,placement,spec)?;
                            moved.replace_leaf_id(new,source).map_err(err)?;
                        }
                        moved.validate().map_err(err)?;*tree=moved;
                    }
                }
            }
            _=>return Err(format!("Unknown layout operation: {op}")),
        }
        Ok(layout.frame())
    }))
}
