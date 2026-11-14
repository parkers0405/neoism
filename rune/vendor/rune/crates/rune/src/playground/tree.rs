//! The story list as a tree: a story's path names its groups, one per `/`, so
//! `"Sidebar/Filter buttons/Compact"` lists "Compact" in "Filter buttons", inside "Sidebar".
//! A group is known by its path prefix (`"Sidebar/Filter buttons"`), which folds keep, and
//! every level folds.

use super::*;

impl<S: 'static> Playground<S> {
    /// Lists these top-level groups first, in this order, then the rest in the order they first
    /// appear. Scenes and stories are alike here: `&["Prototypes", "Scenes"]` lists the
    /// "Prototypes" stories above the scenes. Inside a group, stories keep the order they're
    /// declared in. A name that isn't a group is passed over (and logged in debug builds).
    pub fn group_order(mut self, groups: &[&'static str]) -> Self {
        self.group_order = groups.to_vec();
        self
    }
}

impl<S: 'static> Live<S> {
    /// Story `i`'s groups, outermost first, each by its path prefix: `"Sidebar"`, then
    /// `"Sidebar/Filter buttons"`.
    pub(super) fn groups_of(&self, i: usize) -> Vec<&'static str> {
        let group = self.group(i);
        group.match_indices('/').map(|(at, _)| &group[..at]).chain(std::iter::once(group)).collect()
    }

    /// Stories in list order: each group gathered where its first story is, at every level, and
    /// stories and groups inside one kept in the order they first appear, except that the
    /// top-level groups `first` names come first, in its order ([`Playground::group_order`]).
    pub(super) fn tree_order(&self, listed: &[usize], first: &[&'static str]) -> Vec<usize> {
        if cfg!(debug_assertions) {
            for name in first.iter().filter(|&&name| !listed.iter().any(|&i| self.groups_of(i)[0] == name)) {
                eprintln!("rune: Playground::group_order names {name:?}, which isn't a group in the list");
            }
        }
        let mut out = Vec::with_capacity(listed.len());
        self.gather(listed, 0, first, &mut out);
        out
    }

    fn gather(&self, items: &[usize], depth: usize, first: &[&'static str], out: &mut Vec<usize>) {
        // A story here, or a group's stories, in the order they first appear.
        let mut runs: Vec<(Option<&'static str>, Vec<usize>)> = Vec::new();
        for &i in items {
            match self.groups_of(i).get(depth).copied() {
                Some(group) => match runs.iter_mut().find(|(g, _)| *g == Some(group)) {
                    Some((_, inside)) => inside.push(i),
                    None => runs.push((Some(group), vec![i])),
                },
                None => runs.push((None, vec![i])),
            }
        }
        if depth == 0 { runs.sort_by_key(|(g, _)| g.and_then(|g| first.iter().position(|&f| f == g)).unwrap_or(usize::MAX)); }
        for (group, inside) in runs {
            match group {
                Some(_) => self.gather(&inside, depth + 1, first, out),
                None => out.extend(inside),
            }
        }
    }

    /// What the filter field holds, trimmed.
    pub(super) fn query(&self) -> &str {
        self.controls.filter.text.trim()
    }

    /// Empties the filter field; true when it held something.
    pub(crate) fn clear_filter(&mut self) -> bool {
        if self.controls.filter.text.is_empty() { return false; }
        self.controls.filter.replace_text(String::new());
        true
    }

    /// The first story the filter keeps, unless the selected one is among them.
    pub(crate) fn first_match(&self) -> Option<usize> {
        if self.header.is_none() && self.selected < self.count() && self.matches(self.selected) { return None; }
        self.order.iter().copied().find(|&i| self.matches(i))
    }

    /// The first story the list shows, which ↓ from the filter goes to.
    pub(crate) fn first_shown(&self) -> Option<usize> {
        self.rows().into_iter().find_map(|r| match r {
            Row::Story(i, _) => Some(i),
            Row::Header(..) => None,
        })
    }

    /// Whether story `i` matches the filter: each word somewhere in its path, ignoring case.
    pub(super) fn matches(&self, i: usize) -> bool {
        let query = self.query();
        if query.is_empty() { return true; }
        let path = self.path(i).to_lowercase();
        query.to_lowercase().split_whitespace().all(|word| path.contains(word))
    }

    /// Whether `group` is folded, as the list shows it: a filter shows every match.
    pub(super) fn is_folded(&self, group: &str) -> bool {
        self.query().is_empty() && self.folded.contains(&group)
    }

    /// Whether story `i` is in `group`, directly or in a group inside it.
    pub(super) fn is_in(&self, i: usize, group: &str) -> bool {
        let own = self.group(i);
        own == group || own.strip_prefix(group).is_some_and(|rest| rest.starts_with('/'))
    }

    /// The stories in `group` that the filter keeps, which its header counts.
    pub(super) fn count_in(&self, group: &str) -> usize {
        self.order.iter().filter(|&&i| self.is_in(i, group) && self.matches(i)).count()
    }

    /// The first story in `group`, filter or not, which keys its header.
    fn first_in(&self, group: &str) -> usize {
        self.order.iter().copied().find(|&i| self.is_in(i, group)).unwrap_or(0)
    }

    /// The group around `group`, if it's not at the top.
    pub(super) fn parent(group: &'static str) -> Option<&'static str> {
        group.rfind('/').map(|at| &group[..at])
    }

    /// The list's rows: each group's header, then what's in it unless it, or a group around it,
    /// is folded. A filter keeps its matches and the headers above them.
    pub(super) fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut open: Vec<&'static str> = Vec::new();
        for &i in self.order.iter().filter(|&&i| self.matches(i)) {
            let groups = self.groups_of(i);
            let same = open.iter().zip(&groups).take_while(|(a, b)| a == b).count();
            open.truncate(same);
            for depth in same..groups.len() {
                if !groups[..depth].iter().any(|g| self.is_folded(g)) { rows.push(Row::Header(groups[depth], self.first_in(groups[depth]), depth)); }
                open.push(groups[depth]);
            }
            if !groups.iter().any(|g| self.is_folded(g)) { rows.push(Row::Story(i, groups.len())); }
        }
        rows
    }

    /// The outermost folded group that hides story `i`'s row, or `group`'s header, if one does.
    fn hidden_by(&self, groups: &[&'static str]) -> Option<&'static str> {
        groups.iter().copied().find(|g| self.is_folded(g))
    }

    /// After folds change: a selection whose row a fold hid moves to the header that hides it.
    pub(super) fn settle(&mut self) {
        if !self.open { return; }
        let hider = match self.header {
            Some(h) => {
                let mut around: Vec<&'static str> = std::iter::successors(Self::parent(h), |g| Self::parent(g)).collect();
                around.reverse();
                self.hidden_by(&around)
            }
            None if self.selected < self.count() => self.hidden_by(&self.groups_of(self.selected)),
            None => None,
        };
        if let Some(g) = hider { self.header = Some(g); }
    }

    /// Every group in the list, outermost first, as folds name them.
    pub(super) fn all_groups(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for &i in &self.order {
            for g in self.groups_of(i) {
                if !out.contains(&g) { out.push(g); }
            }
        }
        out
    }
}

/// A row of the story list. A header carries its first story, which keys it, and its depth;
/// a story its depth, the number of groups it's in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Row {
    Header(&'static str, usize, usize),
    Story(usize, usize),
}

impl Row {
    /// A header's element key: by its first story, with its depth below the top.
    pub(super) fn header_key(first: usize, depth: usize) -> String {
        if depth == 0 { format!("playground-group-{first}") } else { format!("playground-group-{first}-{depth}") }
    }
}
