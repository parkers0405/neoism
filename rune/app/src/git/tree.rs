//! Changed files as a folder tree, for the panel's list and the diff's file list: folders
//! fold, and a folder holding only one folder is shown as one row (`src/agent`).

use super::*;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Node {
    Dir { path: String, name: String, depth: usize, open: bool },
    /// `index` into the files the tree was built from.
    File { index: usize, depth: usize },
}

/// The visible rows: folders first-seen in path order, files under them, closed folders' contents left out.
pub(super) fn rows<'a>(paths: impl Iterator<Item = &'a str>, closed: &HashSet<String>) -> Vec<Node> {
    let paths: Vec<&str> = paths.collect();
    let mut order: Vec<usize> = (0..paths.len()).collect();
    // Folders before files at each level, each alphabetical, as the file tree sorts.
    let key = |i: &usize| { let p = paths[*i]; let (dir, name) = p.rsplit_once('/').map_or(("", p), |(d, n)| (d, n)); (dir.to_lowercase(), name.to_lowercase()) };
    order.sort_by(|a, b| {
        let (pa, pb): (Vec<&str>, Vec<&str>) = (paths[*a].split('/').collect(), paths[*b].split('/').collect());
        for depth in 0.. {
            match (pa.get(depth), pb.get(depth)) {
                (Some(x), Some(y)) if x == y => continue,
                (Some(x), Some(y)) => {
                    let (x_dir, y_dir) = (depth + 1 < pa.len(), depth + 1 < pb.len());
                    return y_dir.cmp(&x_dir).then_with(|| x.to_lowercase().cmp(&y.to_lowercase()));
                }
                _ => break,
            }
        }
        key(a).cmp(&key(b))
    });
    // How many distinct children each folder has, to merge single-folder chains.
    let mut children: std::collections::HashMap<&str, HashSet<&str>> = Default::default();
    for path in &paths {
        let mut at = 0;
        while let Some(next) = path[at..].find('/') {
            let end = at + next;
            let child_end = path[end + 1..].find('/').map_or(path.len(), |n| end + 1 + n);
            children.entry(&path[..end]).or_default().insert(&path[..child_end]);
            at = end + 1;
        }
    }
    let lone_dir = |dir: &str| children.get(dir).is_some_and(|c| c.len() == 1 && c.iter().all(|child| children.contains_key(child)));
    let mut out = Vec::new();
    let mut shown: Vec<String> = Vec::new(); // the folder rows open above the current file
    for index in order {
        let path = paths[index];
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
        // The folder rows this file sits under: one per folder, chains merged into their last.
        let mut wanted: Vec<(String, String)> = Vec::new();
        let (mut at, mut label_from) = (0, 0);
        while at < dir.len() {
            let end = dir[at..].find('/').map_or(dir.len(), |n| at + n);
            if !lone_dir(&dir[..end]) || end == dir.len() {
                wanted.push((dir[..end].to_owned(), dir[label_from..end].to_owned()));
                label_from = end + 1;
            }
            at = end + 1;
        }
        let common = shown.iter().zip(&wanted).take_while(|(a, b)| **a == b.0).count();
        shown.truncate(common);
        let mut hidden = shown.iter().any(|d| closed.contains(d));
        for (path, name) in wanted.into_iter().skip(common) {
            if !hidden { out.push(Node::Dir { open: !closed.contains(&path), depth: shown.len(), name, path: path.clone() }); }
            hidden |= closed.contains(&path);
            shown.push(path);
        }
        if !hidden { out.push(Node::File { index, depth: shown.len() }); }
    }
    out
}

/// A folder's or a file's row, drawn as the file tree draws its own: chevron, icon, name.
pub(super) fn label(app: &App, node: &Node, name: &str, ink: u32) -> El {
    use crate::ui::{icons, theme::NERD};
    let p = app.pal;
    let s = app.ty.scale();
    let (depth, chevron, glyph, color) = match node {
        Node::Dir { depth, open, .. } => (*depth, Some(if *open { "\u{f078}" } else { "\u{f054}" }), if *open { icons::FOLDER_OPEN } else { icons::FOLDER_CLOSED }, p.folder),
        Node::File { depth, .. } => { let (glyph, color) = icons::for_file(name); (*depth, None, glyph, color) }
    };
    row().h_full().grow(1.0).shrink().center_y().pl(depth as f32 * 14.0 * s)
        .child(el().w(14.0 * s).center().children(chevron.map(|c| text(c).font(rune::TextStyle::new(8.0 * s, 400.0).family(NERD)).color(rgb(p.muted)))))
        .child(el().w(16.0 * s).center().child(text(glyph).font(rune::TextStyle::new(13.0 * s, 400.0).family(NERD)).color(rgb(color))))
        .child(text(name.to_owned()).font(app.ty.sans(12.5, if matches!(node, Node::Dir { .. }) { 500.0 } else { 400.0 })).color(rgb(ink)).ml(7.0 * s).truncate().grow(1.0).shrink())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(rows: &[Node], paths: &[&str]) -> Vec<String> {
        rows.iter().map(|n| match n {
            Node::Dir { name, depth, open, .. } => format!("{}{name}/{}", "  ".repeat(*depth), if *open { "" } else { " (closed)" }),
            Node::File { index, depth } => format!("{}{}", "  ".repeat(*depth), paths[*index].rsplit('/').next().unwrap()),
        }).collect()
    }

    #[test]
    fn files_nest_under_folders_and_single_folder_chains_merge() {
        let paths = ["README.md", "rune/app/src/git.rs", "rune/app/src/git/tree.rs", "rune/app/src/main.rs", "docs/a.md"];
        let rows = rows(paths.iter().copied(), &HashSet::new());
        assert_eq!(names(&rows, &paths), [
            "docs/", "  a.md",
            "rune/app/src/", "  git/", "    tree.rs", "  git.rs", "  main.rs",
            "README.md",
        ]);
    }

    #[test]
    fn a_closed_folder_hides_what_is_in_it() {
        let paths = ["src/a.rs", "src/git/b.rs", "z.txt"];
        let closed: HashSet<String> = ["src".to_owned()].into();
        let rows = rows(paths.iter().copied(), &closed);
        assert_eq!(names(&rows, &paths), ["src/ (closed)", "z.txt"]);
        let closed: HashSet<String> = ["src/git".to_owned()].into();
        assert_eq!(names(&super::rows(paths.iter().copied(), &closed), &paths), ["src/", "  git/ (closed)", "  a.rs", "z.txt"]);
    }
}
