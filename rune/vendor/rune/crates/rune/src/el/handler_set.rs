//! Event, gesture, layout-watch and pointer-feedback handlers shared by `El` and `Node` behind
//! `Cold<Handlers>`, so elements without callbacks pay one pointer.
use crate::cold::Empty;
use crate::el::{
    AtBottomFn, Cursor, DragEndFn, DragFn, DragOutFn, DropSpec, LanguagesFn, LayoutFn, LayoutTxFn, ModalSpec,
    PasteFn, PointerFn, PressFn, ScrollOffsetFn, SelectionChangeFn, SelectionCopy, VisibleRowsFn,
    WindowFocusFn,
};

/// How direct touch arbitrates a custom drag against native viewport panning.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum TouchPolicy {
    #[default]
    Drag,
    Scroll,
    HoldDrag,
}

#[derive(Clone)]
pub(crate) struct Handlers {
    pub on_paste: Option<PasteFn>,
    pub on_window_focus: Option<WindowFocusFn>,
    /// `El::on_spellcheck`: the app's spell checking setting and what flips it.
    pub on_spellcheck: Option<(bool, WindowFocusFn)>,
    /// `El::on_spellcheck_languages`: keeps the dictionaries a field's menu chose.
    pub on_spellcheck_languages: Option<LanguagesFn>,
    pub modal: Option<ModalSpec>,
    pub on_press: Option<PressFn>,
    pub on_double_press: Option<PressFn>,
    pub on_secondary_press: Option<PointerFn>,
    pub on_hover: Option<PressFn>,
    pub on_hover_change: Option<WindowFocusFn>,
    /// Seconds the pointer must stay inside, then outside, before `on_hover_change` hears it.
    pub hover_delay: [f32; 2],
    pub on_pointer_move: Option<PointerFn>,
    /// `on_wheel`, `on_magnify` and the other canvas handlers (`canvas_input.rs`).
    pub gestures: Option<Box<crate::input::canvas_input::CanvasInput>>,
    pub on_drag: Option<DragFn>,
    pub touch_policy: TouchPolicy,
    pub drag_out: Option<DragOutFn>,
    pub drag_threshold: Option<f32>,
    pub on_drag_start: Option<PressFn>,
    pub on_drag_end: Option<DragEndFn>,
    pub drop: Option<Box<DropSpec>>,
    pub on_at_bottom: Option<AtBottomFn>,
    pub on_far_from_bottom: Option<(f32, AtBottomFn)>,
    pub on_visible_rows: Option<VisibleRowsFn>,
    pub on_layout: Option<LayoutTxFn>,
    pub on_screen_layout: Option<LayoutFn>,
    pub on_drawn_layout: Option<LayoutTxFn>,
    /// `El::on_visible`: the element came on screen (`true`) or left it.
    pub on_visible: Option<WindowFocusFn>,
    /// `El::drawn_rect_into`: where each frame draws the element.
    pub drawn_rect: Option<crate::Live<[f32; 4]>>,
    pub on_scroll_offset: Option<ScrollOffsetFn>,
    pub selection_copy: Option<SelectionCopy>,
    pub on_selection_change: Option<SelectionChangeFn>,
    pub cursor: Option<Cursor>,
    pub press_scale: Option<f32>,
    /// Debug builds: where the app set the handlers the commit warnings name.
    #[cfg(debug_assertions)]
    pub sites: HandlerSites,
}

#[cfg(debug_assertions)]
type Site = Option<&'static std::panic::Location<'static>>;

/// Where `on_hover`, `on_hover_change`, `on_pointer_move`, `on_layout`, `on_drawn_layout` and
/// `on_visible_rows` were set.
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Default)]
pub(crate) struct HandlerSites {
    pub hover: Site,
    pub hover_change: Site,
    pub pointer_move: Site,
    pub layout: Site,
    pub drawn_layout: Site,
    pub visible_rows: Site,
}

impl Handlers {
    #[inline]
    pub(crate) fn has_primary(&self) -> bool {
        self.on_press.is_some()
            || self.on_double_press.is_some()
            || self.on_hover.is_some()
            || self.on_drag.is_some()
            || self.drag_out.is_some()
    }

    #[inline]
    pub(crate) fn takes_hit(&self) -> bool {
        self.on_pointer_move.is_some()
            || self.on_press.is_some()
            || self.on_double_press.is_some()
            || self.on_secondary_press.is_some()
            || self.on_hover.is_some()
            || self.on_hover_change.is_some()
            || self.on_drag.is_some()
            || self.drag_out.is_some()
            || self.modal.is_some()
            || self.gestures.as_ref().is_some_and(|g| g.takes_gestures())
    }
}

impl Empty for Handlers {
    const EMPTY: &'static Self = &Handlers {
        on_paste: None,
        on_window_focus: None,
        on_spellcheck: None,
        on_spellcheck_languages: None,
        modal: None,
        on_press: None,
        on_double_press: None,
        on_secondary_press: None,
        on_hover: None,
        on_hover_change: None,
        hover_delay: [0.0; 2],
        on_pointer_move: None,
        gestures: None,
        on_drag: None,
        touch_policy: TouchPolicy::Drag,
        drag_out: None,
        drag_threshold: None,
        on_drag_start: None,
        on_drag_end: None,
        drop: None,
        on_at_bottom: None,
        on_far_from_bottom: None,
        on_visible_rows: None,
        on_layout: None,
        on_screen_layout: None,
        on_drawn_layout: None,
        on_visible: None,
        drawn_rect: None,
        on_scroll_offset: None,
        selection_copy: None,
        on_selection_change: None,
        cursor: None,
        press_scale: None,
        #[cfg(debug_assertions)]
        sites: HandlerSites { hover: None, hover_change: None, pointer_move: None, layout: None, drawn_layout: None, visible_rows: None },
    };
}
