//! `Tx`'s scroll requests: revealing a row, moving a viewport to its end, a page or an
//! offset, and holding an element where it shows.

#[cfg(doc)]
use crate::El;
use crate::Tx;

impl Tx {
    /// Reveal a keyed row, including a currently unrealized virtual row. It scrolls the nearest
    /// vertical viewport around the row and, through [`El::scroll_x`] or [`El::scroll_xy`], the
    /// nearest horizontal one, each just far enough. The vertical reveal follows the presented
    /// row while layout or growth moves it; wheel input, a fling or another scroll request wins.
    ///
    /// `key` names an element as [`Cx::layout`](crate::Cx::layout) does, here and in the other
    /// scroll requests: a key (the first element with it), a full path, or a key path such as
    /// `"inspector/label"` whose leading keys are the element's nearest keyed ancestors, which
    /// tells apart an app's `label` from a component's.
    pub fn scroll_into_view(&mut self, key: impl Into<String>) {
        self.scroll_to.push(crate::runtime::ScrollReveal::into_view(None, key.into()));
    }
    /// [`Tx::scroll_into_view`] for row `key` inside the element keyed `scope` only, so lists
    /// that key their rows alike (a file tree and a diff, both by path) don't reveal each
    /// other's rows.
    pub fn scroll_into_view_in(&mut self, scope: impl Into<String>, key: impl Into<String>) {
        self.scroll_to.push(crate::runtime::ScrollReveal::into_view(Some(scope.into()), key.into()));
    }
    /// Reveal and vertically centre a keyed element, virtual row or [`crate::TextHighlight`],
    /// gliding on the spring [`Tx::scroll_to`] uses (a jump under [`Tx::instant`], reduced
    /// motion or a settled frame). `"row/mark"` mounts virtual row `row` first and centres
    /// `mark` inside it.
    pub fn scroll_center(&mut self, key: impl Into<String>) {
        self.scroll_to.push(crate::runtime::ScrollReveal::center(None, key.into(), true));
    }
    /// [`Tx::scroll_center`] inside the element keyed `scope` only.
    pub fn scroll_center_in(&mut self, scope: impl Into<String>, key: impl Into<String>) {
        self.scroll_to.push(crate::runtime::ScrollReveal::center(Some(scope.into()), key.into(), true));
    }
    /// [`Tx::scroll_center_in`] without the glide.
    pub fn scroll_center_in_instant(&mut self, scope: impl Into<String>, key: impl Into<String>) {
        self.scroll_to.push(crate::runtime::ScrollReveal::center(Some(scope.into()), key.into(), false));
    }
    /// Glide the scroll viewport keyed `key` (or the one containing it) to its bottom, as a
    /// "jump to latest" button does. A `stick_bottom` viewport follows the end again once it
    /// lands. Long distances cut to two viewports from the end first; reduced motion jumps.
    pub fn scroll_to_bottom(&mut self, key: impl Into<String>) {
        self.scroll_bottom.push((key.into(), true));
    }
    /// [`Tx::scroll_to_bottom`] without the glide.
    pub fn scroll_to_bottom_instant(&mut self, key: impl Into<String>) {
        self.scroll_bottom.push((key.into(), false));
    }
    /// Page the scroll viewport keyed `key` (or the one containing it) by `pages` pages, down
    /// for positive, as Page Up and Page Down do: a page is 90% of its height, the move is
    /// instant and clamped, and a `stick_bottom` viewport that lands on its end follows it
    /// again. An app routes the page keys itself with it, e.g. from a composer's
    /// `on_input_key` to the message list.
    pub fn scroll_page(&mut self, key: impl Into<String>, pages: i32) {
        self.scroll_pages.push((key.into(), pages));
    }
    /// Move the horizontal scroll viewport keyed `key` (or the nearest one containing it) so it
    /// shows its content from `x` points in, clamped to its ends: 0 is the start, as a carousel's
    /// "back to the first" does. Instant; any momentum stops. See [`El::scroll_x`].
    pub fn scroll_to_x(&mut self, key: impl Into<String>, x: f32) {
        self.scroll_offsets.push(crate::runtime::ScrollRequest { key: key.into(), x: Some(x), y: None, glide: None, anchor: false });
    }
    /// Move the vertical scroll viewport keyed `key` (or the nearest one containing it) so it
    /// shows its content from `y` points down, clamped to its ends. Instant; any momentum or
    /// glide stops, and a `stick_bottom` viewport that lands on its end follows it again.
    pub fn scroll_to_y(&mut self, key: impl Into<String>, y: f32) {
        self.scroll_offsets.push(crate::runtime::ScrollRequest { key: key.into(), x: None, y: Some(y), glide: None, anchor: false });
    }
    /// Glide the scroll viewport keyed `key` (or the nearest one containing it) to show its
    /// content from (`x`, `y`) points in, on the spring [`Tx::scroll_to_bottom`] uses, as a
    /// "focus" or "back to the start" button does. Each offset is clamped to the content,
    /// rechecked as the glide runs (virtual rows measure as they mount); axes the viewport
    /// doesn't scroll are ignored, so a vertical list takes `y` alone. The wheel, a fling or
    /// another request takes over mid-glide, a request keeping its speed. Reduced motion, and a
    /// settled frame, jump. [`Tx::scroll_to_with`] picks the spring.
    pub fn scroll_to(&mut self, key: impl Into<String>, x: f32, y: f32) {
        self.scroll_offsets.push(crate::runtime::ScrollRequest { key: key.into(), x: Some(x), y: Some(y), glide: Some(crate::tok::motion::GLIDE), anchor: false });
    }
    /// [`Tx::scroll_to`] on `spring` instead of the default glide. A request that arrives
    /// mid-glide keeps the glide's speed, so a selection moved by key repeat, each press asking
    /// for the next row, scrolls in one continuous motion rather than in steps. A quicker spring
    /// covers more of the way on the first frame:
    ///
    /// ```
    /// # let mut tx = rune::Tx::default();
    /// # let row_top = 640.0;
    /// // ↓ moved the selection: keep its row in view, on a quicker spring than the default.
    /// tx.scroll_to_with("board", 0.0, row_top, rune::Spring::new(0.18, 0.0));
    /// ```
    pub fn scroll_to_with(&mut self, key: impl Into<String>, x: f32, y: f32, spring: crate::Spring) {
        self.scroll_offsets.push(crate::runtime::ScrollRequest { key: key.into(), x: Some(x), y: Some(y), glide: Some(spring), anchor: false });
    }
    /// [`Tx::scroll_to`] without the glide. An app stepping its own glide calls it each frame:
    /// hover under a still pointer then holds while the content moves and resolves once the
    /// frame after the last step, as it does for a glide.
    pub fn scroll_to_instant(&mut self, key: impl Into<String>, x: f32, y: f32) {
        self.scroll_offsets.push(crate::runtime::ScrollRequest { key: key.into(), x: Some(x), y: Some(y), glide: None, anchor: false });
    }
    /// Move the vertical scroll viewport around the element or [`crate::TextHighlight`] keyed
    /// `key` so its top sits `at` points below the viewport's top, clamped to the content's ends:
    /// a page that opens on a heading lands it just under its bar. It's placed from the layout of the commit this
    /// handler causes, so the frame that first draws a new page draws it there, and unlike
    /// [`Tx::scroll_into_view`] where it lands doesn't depend on where the view was or what was
    /// stuck over it. Instant; any momentum or glide stops. The element must be laid out: reveal
    /// a virtual row that isn't mounted with [`Tx::scroll_into_view`].
    pub fn scroll_key_to(&mut self, key: impl Into<String>, at: f32) {
        self.scroll_offsets.push(crate::runtime::ScrollRequest { key: key.into(), x: None, y: Some(at), glide: None, anchor: true });
    }
    /// Keep the element keyed `key` (or at that path) where it shows on screen across the
    /// commit this handler causes, by moving the nearest vertical scroll viewport around it. A
    /// stuck [`El::sticky_top`] header that collapses its section stays under the pointer
    /// instead of jumping up with the content, and one that expands stays put as its rows open
    /// below it. Its top edge is what holds, measured where the last frame drew it, stuck shift
    /// included; the offset is clamped to the content, and a `stick_bottom` viewport that lands
    /// on its end follows it again. Scroll compensation follows the drawn element while
    /// layout and growth springs run; other rows keep animating. A scroll asked for in the
    /// same handler, or subsequent wheel input, wins.
    pub fn keep_on_screen(&mut self, key: impl Into<String>) {
        self.keep_on_screen.push(key.into());
    }
}
