//! Sibling key checks and repair. They run on the view before virtualization, so offscreen
//! siblings are checked too; a `virtualize` list's own rows are checked as it windows them.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use crate::El;

/// A sibling key used twice under one parent.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Duplicate {
    pub key: String,
    pub message: String,
}

/// Whether no two of these siblings share a key. A hash collision can say false for unique
/// keys, which only sends the caller to the exact check. Allocates nothing once `hashes` has
/// grown to the widest parent.
fn unique(shared: &[El], own: &[El], hashes: &mut Vec<u64>) -> bool {
    let keys = shared.iter().chain(own).filter_map(|c| c.key.as_deref());
    let n = keys.clone().count();
    if n < 2 { return true; }
    if n <= 8 {
        return keys.clone().enumerate().all(|(i, a)| keys.clone().skip(i + 1).all(|b| a != b));
    }
    hashes.clear();
    hashes.extend(keys.map(|k| {
        let mut h = rustc_hash::FxHasher::default();
        k.hash(&mut h);
        h.finish()
    }));
    hashes.sort_unstable();
    hashes.windows(2).all(|w| w[0] != w[1])
}

/// The sibling sets under `el` that need checking: a memo's shared children were checked when
/// it built, and a virtual list's rows are checked as it windows them.
fn checks_siblings(el: &El) -> bool {
    el.cold.virtual_estimate.is_none() && !(el.memo_children().is_some() && el.children.is_empty())
}

fn shared_children(el: &El) -> &[El] {
    el.memo_children().map_or(&[], |m| &m.children)
}

fn clean_with(el: &El, hashes: &mut Vec<u64>) -> bool {
    if el.memo_children().is_some_and(|m| m.duplicate_keys) { return false; }
    if checks_siblings(el) && !unique(shared_children(el), &el.children, hashes) { return false; }
    el.children.iter().all(|c| clean_with(c, hashes))
}

/// Whether no parent in `root` has two children with one key: the per-commit check, which
/// allocates nothing and skips the subtrees of memos (checked once, when they build).
pub(crate) fn clean(root: &El) -> bool {
    thread_local! { static HASHES: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) }; }
    HASHES.with_borrow_mut(|hashes| clean_with(root, hashes))
}

/// Whether a memo's children, as built, hold duplicate keys anywhere below the memo's root.
pub(crate) fn memo_children_have_duplicates(root: &El, children: &[El]) -> bool {
    let mut hashes = Vec::new();
    (root.cold.virtual_estimate.is_none() && !unique(children, &[], &mut hashes))
        || !children.iter().all(|c| clean_with(c, &mut hashes))
}

/// The duplicates under `root`, with messages naming both call sites. Only for trees [`clean`]
/// rejected: paths are built only down to the duplicates.
fn find(root: &El, path: &str) -> Vec<Duplicate> {
    fn walk(el: &El, path: &str, hashes: &mut Vec<u64>, out: &mut Vec<Duplicate>) {
        let shared = el.memo_children().is_some_and(|m| m.duplicate_keys);
        if checks_siblings(el) || (shared && el.cold.virtual_estimate.is_none()) {
            let mut seen: HashMap<&str, &El> = HashMap::new();
            for child in el.all_children() {
                let Some(key) = &child.key else { continue };
                if let Some(first) = seen.get(key.as_str()) {
                    out.push(Duplicate {
                        key: key.clone(),
                        message: format!(
                            "duplicate sibling key {key:?} under {path:?}: first at {}, again at {}; give siblings unique keys",
                            first.key_location(), child.key_location(),
                        ),
                    });
                } else {
                    seen.insert(key, child);
                }
            }
        }
        for (index, child) in el.all_children().enumerate() {
            if clean_with(child, hashes) { continue; }
            let segment = child.key.clone().unwrap_or_else(|| format!("#{index}"));
            walk(child, &format!("{path}/{segment}"), hashes, out);
        }
    }
    let mut out = Vec::new();
    walk(root, path, &mut Vec::new(), &mut out);
    out
}

fn root_path(root: &El) -> &str {
    root.key.as_deref().unwrap_or("#0")
}

#[cfg(test)]
pub(crate) fn warnings(root: &El) -> Vec<String> {
    find(root, root_path(root)).into_iter().map(|d| d.message).collect()
}

/// Renames each later copy of a key `key#dup1`, `key#dup2`, ... in order, skipping names
/// already taken, so the first keeps its key. Returns each renamed index with its old key.
pub(crate) fn rename_duplicates<'a>(keys: impl Iterator<Item = &'a mut String>) -> Vec<(usize, String)> {
    let mut keys: Vec<&mut String> = keys.collect();
    let mut taken: HashSet<String> = keys.iter().map(|k| k.to_string()).collect();
    let mut kept: HashSet<String> = HashSet::new();
    let mut renamed = Vec::new();
    for (i, key) in keys.iter_mut().enumerate() {
        if kept.insert(key.to_string()) { continue; }
        let name = (1..).map(|n| format!("{key}#dup{n}")).find(|k| !taken.contains(k)).unwrap();
        taken.insert(name.clone());
        kept.insert(name.clone());
        renamed.push((i, std::mem::replace(*key, name)));
    }
    renamed
}

/// Makes sibling keys unique so the layout tree never sees two siblings share a node. The first
/// sibling keeps its key; each later one is renamed `key#dup1`, `key#dup2`, ... in tree order, so
/// it still draws and keeps a stable identity across builds. Returns what it renamed; a tree
/// without duplicates costs one allocation-free walk.
pub(crate) fn repair(root: &mut El) -> Vec<Duplicate> {
    if clean(root) { return Vec::new(); }
    let path = root_path(root).to_owned();
    repair_at(root, &path)
}

/// [`repair`] for a subtree whose root sits at `path`.
pub(crate) fn repair_at(root: &mut El, path: &str) -> Vec<Duplicate> {
    if clean(root) { return Vec::new(); }
    let found = find(root, path);
    if found.is_empty() { return found; }
    // Only memos on the way to a duplicate are copied out of the cache.
    fn walk(el: &mut El, hashes: &mut Vec<u64>) {
        let shared = el.memo_children().is_some_and(|m| m.duplicate_keys);
        let siblings = checks_siblings(el) && !unique(shared_children(el), &el.children, hashes);
        if shared || siblings { el.expand_memo(); }
        if siblings || (shared && el.cold.virtual_estimate.is_none()) {
            rename_duplicates(el.children.iter_mut().filter_map(|c| c.key.as_mut()));
        }
        for child in &mut el.children {
            if !clean_with(child, hashes) { walk(child, hashes); }
        }
    }
    walk(root, &mut Vec::new());
    found
}

/// Prints each duplicate once per key for the life of the process.
pub(crate) fn warn_once(found: &[Duplicate]) {
    thread_local! { static WARNED: RefCell<HashSet<String>> = Default::default(); }
    if found.is_empty() { return; }
    WARNED.with_borrow_mut(|warned| {
        for d in found {
            if warned.insert(d.key.clone()) { eprintln!("rune: {}; the later one is renamed {:?}", d.message, format!("{}#dupN", d.key)); }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{col, el};

    #[test]
    fn duplicate_keys_report_both_sources_and_allow_other_parents() {
        let first = el().key("card");
        let second = el().key("card");
        let first_source = first.key_location();
        let second_source = second.key_location();
        let tree = col().key("cards").children([first, second]);
        let found = warnings(&tree);
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("\"card\""));
        assert!(found[0].contains(&first_source));
        assert!(found[0].contains(&second_source));
        #[cfg(debug_assertions)]
        assert_ne!(first_source, second_source);
        assert!(warnings(&col().children([
            col().key("a").child(el().key("card")),
            col().key("b").child(el().key("card")),
            el(), el(),
        ])).is_empty());
    }

    /// A memo's children are checked once, when it builds: a clean memo is skipped and left
    /// shared, and repair copies out only the memo holding the duplicate.
    #[test]
    fn memos_are_checked_when_built_and_only_a_dirty_one_is_expanded() {
        fn memo(key: &str, children: Vec<El>) -> El {
            let mut root = el().key(key);
            let duplicate_keys = memo_children_have_duplicates(&root, &children);
            let children = std::rc::Rc::new(crate::memo::Shared { children, focused_input: false, duplicate_keys, has_virtual: false, reusable: false });
            root.memo = Some(std::rc::Rc::new(crate::memo::Built { id: 0, reusable: false, root: el(), children }));
            root
        }
        let clean_memo = || memo("a", vec![el().key("x"), el().key("y").child(el().key("z"))]);
        let tree = col().children([clean_memo(), memo("b", vec![el().key("x")])]);
        assert!(clean(&tree));
        let dirty = memo("c", vec![el().key("p").children([el().key("x"), el().key("x")])]);
        assert!(dirty.memo_children().unwrap().duplicate_keys);
        let mut tree = col().children([clean_memo(), dirty]);
        assert!(!clean(&tree));
        let found = repair(&mut tree);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].message.contains("under \"#0/c/p\""), "{}", found[0].message);
        assert!(tree.children[0].memo_children().is_some(), "the clean memo stays shared");
        assert!(tree.children[1].memo_children().is_none());
        let keys: Vec<_> = tree.children[1].children[0].children.iter().map(|c| c.key.clone().unwrap()).collect();
        assert_eq!(keys, ["x", "x#dup1"]);
        assert!(clean(&tree));
        // Sibling keys past the short-list compare go through the hashes.
        let wide = |dup: bool| col().children((0..40).map(|i| el().key(if dup && i == 39 { "k3".to_string() } else { format!("k{i}") })));
        assert!(clean(&wide(false)));
        assert!(!clean(&wide(true)));
    }

    /// The per-commit repair of a 3,700-row tree without duplicates allocates nothing.
    #[test]
    fn repairing_a_tree_without_duplicates_allocates_nothing() {
        let row = |i: usize| el().key(format!("row:{i}")).children([el().key("title"), el().key("meta").child(el().key("time"))]);
        let mut tree = col().key("root").children([col().key("rows").children((0..3700).map(row)), el().key("footer")]);
        assert!(repair(&mut tree).is_empty());
        let before = crate::tooling::test_alloc::allocs();
        let t = std::time::Instant::now();
        assert!(repair(&mut tree).is_empty());
        let elapsed = t.elapsed();
        assert_eq!(crate::tooling::test_alloc::allocs() - before, 0);
        eprintln!("clean 3,700-row repair: {elapsed:?}");
    }

    #[test]
    fn repair_keeps_the_first_and_renames_later_duplicates_deterministically() {
        let keys = |el: &El| el.children.iter().map(|c| c.key.clone().unwrap_or_default()).collect::<Vec<_>>();
        let build = || col().children([
            el().key("row:R-94"), el().key("row:R-94#dup1"), el(), el().key("row:R-94"), el().key("row:R-94"),
            col().key("inner").children([el().key("x"), el().key("x")]),
        ]);
        let mut tree = build();
        let found = repair(&mut tree);
        assert_eq!(found.iter().map(|d| d.key.as_str()).collect::<Vec<_>>(), ["row:R-94", "row:R-94", "x"]);
        assert_eq!(keys(&tree), ["row:R-94", "row:R-94#dup1", "", "row:R-94#dup2", "row:R-94#dup3", "inner"]);
        assert_eq!(keys(&tree.children[5]), ["x", "x#dup1"]);
        assert!(warnings(&tree).is_empty());
        let mut again = build();
        repair(&mut again);
        assert_eq!(keys(&again), keys(&tree), "the same tree repairs the same way every build");
        let mut clean = col().children([el().key("a"), el().key("b")]);
        assert!(repair(&mut clean).is_empty());
    }
}
