//! Ordered family navigation and frame-local hit regions for child headers.

use super::state::side_panel::NeoismAgentSessionEntry;
use crate::primitives::visible_rect_fragments;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Previous,
    Next,
}

impl Direction {
    fn index(self) -> usize {
        match self {
            Self::Previous => 0,
            Self::Next => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConversationNavigation<'a> {
    pub previous: Option<&'a str>,
    pub next: Option<&'a str>,
    pub previous_is_main: bool,
}

impl<'a> ConversationNavigation<'a> {
    pub fn target(self, direction: Direction) -> Option<&'a str> {
        match direction {
            Direction::Previous => self.previous,
            Direction::Next => self.next,
        }
    }
}

pub fn navigation_targets<'a>(
    is_subagent: bool,
    viewed: Option<&str>,
    root: Option<&'a str>,
    roster: impl IntoIterator<Item = &'a NeoismAgentSessionEntry>,
) -> ConversationNavigation<'a> {
    let Some(viewed) = viewed.filter(|id| !id.is_empty()) else {
        return ConversationNavigation::default();
    };
    let Some(root) = root.filter(|id| is_subagent && !id.is_empty() && *id != viewed)
    else {
        return ConversationNavigation::default();
    };
    let fallback = ConversationNavigation {
        previous: Some(root),
        next: None,
        previous_is_main: true,
    };
    let mut rows = roster.into_iter();
    if rows.next().map(|row| row.id.as_str()) != Some(root) {
        return fallback;
    }
    let mut previous = root;
    while let Some(row) = rows.next() {
        let id = row.id.as_str();
        if id.is_empty() || id == root {
            continue;
        }
        if id == viewed {
            return ConversationNavigation {
                previous: Some(previous),
                next: rows
                    .find(|row| !row.id.is_empty() && row.id != root && row.id != viewed)
                    .map(|row| row.id.as_str()),
                previous_is_main: previous == root,
            };
        }
        previous = id;
    }
    fallback
}

#[derive(Default)]
struct HeaderLink {
    target: Option<String>,
    regions: Vec<[f32; 4]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderHit<'a> {
    pub direction: Direction,
    pub target: &'a str,
}

#[derive(Default)]
pub struct ConversationHeader {
    links: [HeaderLink; 2],
}

impl ConversationHeader {
    pub fn clear(&mut self) {
        for link in &mut self.links {
            link.target = None;
            link.regions.clear();
        }
    }

    pub fn register(
        &mut self,
        direction: Direction,
        target: &str,
        rect: [f32; 4],
        clip: [f32; 4],
        occlusions: &[[f32; 4]],
    ) {
        let link = &mut self.links[direction.index()];
        link.target = None;
        link.regions.clear();
        if target.is_empty() {
            return;
        }
        link.regions = visible_rect_fragments(rect, clip, occlusions);
        if !link.regions.is_empty() {
            link.target = Some(target.to_owned());
        }
    }

    /// Exact frame-local hit fragments after canonical clipping/occlusion.
    pub fn visible_links(&self) -> impl Iterator<Item = (Direction, &str, &[[f32; 4]])> {
        [Direction::Previous, Direction::Next].into_iter().filter_map(|direction| {
            let link = &self.links[direction.index()];
            Some((direction, link.target.as_deref()?, link.regions.as_slice()))
        })
    }

    pub fn hit_at(&self, x: f32, y: f32) -> Option<HeaderHit<'_>> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        for direction in [Direction::Previous, Direction::Next] {
            let link = &self.links[direction.index()];
            if link.regions.iter().any(|rect| {
                x >= rect[0]
                    && x < rect[0] + rect[2]
                    && y >= rect[1]
                    && y < rect[1] + rect[3]
            }) {
                return link
                    .target
                    .as_deref()
                    .map(|target| HeaderHit { direction, target });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> Vec<NeoismAgentSessionEntry> {
        ["main", "first", "nested", "last"]
            .into_iter()
            .map(|id| NeoismAgentSessionEntry::new(id, id, ""))
            .collect()
    }

    #[test]
    fn conversation_navigation_follows_family_order_without_wrapping() {
        let rows = roster();
        let nav = |viewed| navigation_targets(true, Some(viewed), Some("main"), &rows);
        assert_eq!(
            nav("first"),
            ConversationNavigation {
                previous: Some("main"),
                next: Some("nested"),
                previous_is_main: true
            }
        );
        assert_eq!(
            nav("nested"),
            ConversationNavigation {
                previous: Some("first"),
                next: Some("last"),
                previous_is_main: false
            }
        );
        assert_eq!(
            nav("last"),
            ConversationNavigation {
                previous: Some("nested"),
                next: None,
                previous_is_main: false
            }
        );
        assert_eq!(nav("main"), ConversationNavigation::default());
    }

    #[test]
    fn conversation_navigation_requires_a_child_and_distinct_known_root() {
        let rows = roster();
        for (child, viewed, root) in [
            (false, Some("main"), Some("main")),
            (false, Some("first"), Some("main")),
            (true, Some("main"), Some("main")),
            (true, None, Some("main")),
            (true, Some("first"), None),
            (true, Some("first"), Some("")),
            (true, Some(""), Some("main")),
        ] {
            assert_eq!(
                navigation_targets(child, viewed, root, &rows),
                ConversationNavigation::default()
            );
        }
    }

    #[test]
    fn conversation_navigation_missing_view_or_untrusted_roster_falls_back_to_main() {
        let rows = roster();
        let fallback = ConversationNavigation {
            previous: Some("main"),
            next: None,
            previous_is_main: true,
        };
        assert_eq!(
            navigation_targets(true, Some("missing"), Some("main"), &rows),
            fallback
        );
        assert_eq!(
            navigation_targets(true, Some("nested"), Some("other-main"), &rows),
            ConversationNavigation {
                previous: Some("other-main"),
                next: None,
                previous_is_main: true
            }
        );
        assert_eq!(
            navigation_targets(true, Some("first"), Some("main"), &[]),
            fallback
        );
    }

    #[test]
    fn conversation_navigation_registers_both_sides_and_reclips_independently() {
        let mut header = ConversationHeader::default();
        let clip = [0.0, 0.0, 240.0, 100.0];
        let left = [10.0, 10.0, 60.0, 22.0];
        let right = [160.0, 10.0, 60.0, 22.0];
        header.register(Direction::Previous, "main", left, clip, &[]);
        header.register(Direction::Next, "nested", right, clip, &[]);
        assert_eq!(
            header.hit_at(20.0, 20.0),
            Some(HeaderHit {
                direction: Direction::Previous,
                target: "main"
            })
        );
        assert_eq!(
            header.hit_at(170.0, 20.0),
            Some(HeaderHit {
                direction: Direction::Next,
                target: "nested"
            })
        );
        assert_eq!(header.hit_at(100.0, 20.0), None);
        header.register(Direction::Next, "nested", right, clip, &[right]);
        assert!(header.hit_at(170.0, 20.0).is_none());
        assert!(header.hit_at(20.0, 20.0).is_some());
        header.register(
            Direction::Previous,
            "main",
            left,
            [0.0, 0.0, 15.0, 100.0],
            &[],
        );
        assert!(header.hit_at(20.0, 20.0).is_none());
        assert!(header.hit_at(12.0, 20.0).is_some());
        assert!(header.hit_at(f32::NAN, 20.0).is_none());
        header.clear();
        assert!(header.hit_at(12.0, 20.0).is_none());
    }
}
