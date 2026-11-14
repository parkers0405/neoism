//! A whole `ElementInfo` to build from, so a stub doesn't break when Rune adds a field.
use crate::{Color, ElementInfo};

/// A plain, fully opaque element at rest with no key: `el()` with nothing set, before layout
/// (zero rects). Stubs fill in what they test: `ElementInfo { focused: true, ..ElementInfo::stub("send") }`.
impl Default for ElementInfo {
    fn default() -> Self {
        ElementInfo {
            trace_name: None,
            trace_namespace: None,
            loop_phase: None,
            focusable: false,
            focused: false,
            focus_visible: false,
            inert: false,
            exiting: false,
            path: String::new(),
            key: None,
            role: "group",
            label: None,
            text: None,
            rect: [0.0; 4],
            target: [0.0; 4],
            opacity: 1.0,
            scale: 1.0,
            blur: 0.0,
            radius: 0.0,
            fill: Color::TRANSPARENT,
            text_color: None,
            hover: 0.0,
            press: 0.0,
            moving: false,
            parent: None,
            visible: [0.0; 4],
            revealing: false,
            in_growing_row: false,
            scrolling: false,
            scrolled: [0.0; 2],
            virtual_extent: false,
            live_intent: 0,
            caret: None,
            drag_out: false,
            help: None,
            selected: None,
            level: None,
            expanded: None,
            item_selected: false,
            stuck: 0.0,
        }
    }
}

impl ElementInfo {
    /// The default element keyed `key` (its path is the key too), for a test that stands in for
    /// `Runtime::elements`.
    pub fn stub(key: impl Into<String>) -> Self {
        let key = key.into();
        ElementInfo { path: key.clone(), key: Some(key), ..ElementInfo::default() }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Cx, El, ElementInfo, Env, Runtime, el};

    #[test]
    fn a_stub_matches_what_a_plain_keyed_element_reports() {
        let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| -> El { el().key("send") }, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let real = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("send")).unwrap();
        let stub = ElementInfo { rect: real.rect, target: real.target, visible: real.visible, path: real.path.clone(), ..ElementInfo::stub("send") };
        assert_eq!(format!("{stub:?}"), format!("{real:?}"));
        let focused = ElementInfo { focused: true, ..ElementInfo::stub("send") };
        assert_eq!((focused.key.as_deref(), focused.path.as_str(), focused.focused, focused.opacity), (Some("send"), "send", true, 1.0));
    }
}
