//! Visual builders: fill, corners, borders, shadows, gradients, opacity, blur, transforms,
//! clips and hover groups.

use super::*;

impl El {
    #[inline]
    pub fn fill(mut self, c: Color) -> Self {
        self.set_fill(c);
        self
    }
    /// [`El::fill`] in place.
    #[inline]
    pub fn set_fill(&mut self, c: Color) -> &mut Self {
        self.vis.fill = c;
        self
    }
    #[inline]
    pub fn radius(mut self, r: f32) -> Self {
        self.set_radius(r);
        self
    }
    /// [`El::radius`] in place.
    #[inline]
    pub fn set_radius(&mut self, r: f32) -> &mut Self {
        self.vis.set_radii([r; 4]);
        self
    }
    /// Round the corners with Apple's continuous curve (`CALayerCornerCurve.continuous`, SwiftUI's
    /// `.continuous` style), which eases into the straight edges over about 1.53 times the radius
    /// instead of meeting them at a circular arc. Applies to the fill, border, image, outline,
    /// `clip_rounded` and system material; shadows keep circular corners, like a CALayer
    /// shadow from a `CGPath(roundedRect:)`.
    pub fn continuous_corners(mut self) -> Self {
        self.vis_flags.set(VisualFlags::CONTINUOUS_CORNERS, true);
        self
    }
    /// Per-corner radius: top-left, top-right, bottom-right, bottom-left.
    pub fn corners(mut self, r: [f32; 4]) -> Self {
        self.vis.set_radii(r);
        self
    }
    /// A ring `width` points wide just inside the border box, following the corner radii, as a
    /// CSS `border` draws with `box-sizing: border-box`. It takes no layout space and draws over
    /// the fill, which runs under it to the outer edge, so a translucent border on a filled box
    /// shows the fill through it, as in CSS. Inset [`Self::box_shadow`]s draw between the two,
    /// inside the ring. The width snaps to device pixels as a CSS border's does: under one pixel
    /// it draws one, above that it floors to whole pixels. For a ring at its exact fractional
    /// width, as CSS `box-shadow: inset 0 0 0 0.5px` draws, use an inset [`Self::box_shadow`]
    /// with that spread.
    #[inline]
    pub fn border(mut self, width: f32, c: Color) -> Self {
        self.set_border(width, c);
        self
    }
    /// [`El::border`] in place.
    #[inline]
    pub fn set_border(&mut self, width: f32, c: Color) -> &mut Self {
        self.vis.set_border(width, c);
        if let Some(cold) = self.vis.cold.get_mut_if_some() {
            cold.border_bottom = None;
            cold.border_sides = None;
        }
        self
    }
    /// A border with its own width on each side, `[top, right, bottom, left]` points, in one
    /// colour, as CSS `border-top`, `border-right`, `border-bottom` and `border-left` draw: a
    /// zero side draws nothing, and the ring follows the corner radii, narrowing round a corner
    /// from one side's width to the other's. Drawn like [`Self::border`], over the fill and
    /// taking no layout space, with each width snapped to device pixels the same way. So a
    /// `border-bottom: 0.5px` hairline is `.border_sides([0.0, 0.0, 0.5, 0.0], c)`, its one
    /// device pixel the box's last row. Dashes and gradients draw the widest side all round.
    /// Replaces any `border`, and `border` replaces it.
    pub fn border_sides(mut self, widths: [f32; 4], c: Color) -> Self {
        let widths = widths.map(|w| if w.is_finite() { w.max(0.0) } else { 0.0 });
        self.vis.set_border(widths.into_iter().fold(0.0, f32::max), c);
        let uniform = widths.iter().all(|&w| w == widths[0]);
        if !uniform || self.vis.cold.get_mut_if_some().is_some() {
            let cold = self.vis.cold_mut();
            cold.border_bottom = None;
            cold.border_sides = (!uniform).then_some(widths);
        }
        self
    }
    /// Makes this element's [`Self::border`] or [`Self::border_sides`] take room in layout, as a
    /// CSS border does: its padding and content lay out inside it, each side as wide as it
    /// draws, snapped as Chromium snaps a border width to device pixels. So a
    /// `border-bottom: 0.5px` row is one whole device pixel taller at any scale (0.8 points at
    /// 1.25x), and a stack of them ends where a web page's does.
    pub fn border_layout(mut self) -> Self {
        self.vis.cold_mut().border_layout = true;
        self
    }
    /// Draws a colour icon (one whose body names its own colours, such as a file-type badge) as
    /// its silhouette in this element's colour, as an SVG mask of the icon filled with
    /// `currentColor` draws it (`@opencode/ui` FileIcon's `mono`): a file tab's badge in one
    /// colour until the tab is selected or hovered. A monochrome icon draws as it always does.
    pub fn icon_one_color(mut self) -> Self {
        self.vis.cold_mut().icon_one_color = true;
        self
    }
    /// A border whose colour runs from `top` at the top edge to `bottom` at the bottom edge,
    /// e.g. a top-lit hairline rim. Drawn like [`El::border`], inside the box over the fill.
    /// Replaces any `border`, and `border` replaces it.
    pub fn border_gradient(mut self, width: f32, top: Color, bottom: Color) -> Self {
        self.vis.set_border(width, top);
        let cold = self.vis.cold_mut();
        cold.border_bottom = Some(bottom);
        cold.border_sides = None;
        self
    }
    /// Dashes the border (`border` or `border_gradient`): `dash` points on, `gap` points off,
    /// with butt ends, along the ring's centre line, round the corners too. The pattern starts
    /// with a dash where the top edge leaves the top-left corner and runs clockwise. The period
    /// stretches or shrinks a little so a whole number of dashes fits, and the last gap meets the
    /// first dash cleanly. The fill shows through the gaps, and the dashes draw over it as a
    /// solid border does. A zero `dash` or `gap` keeps the border solid.
    pub fn border_dash(mut self, dash: f32, gap: f32) -> Self {
        let valid = dash.is_finite() && gap.is_finite() && dash > 0.0 && gap > 0.0;
        let v = valid.then_some([dash, gap]);
        if v.is_some() || !self.vis.cold.is_empty() {
            self.vis.cold_mut().border_dash = v;
        }
        self
    }
    /// Starts the dash pattern `phase` points into itself (SwiftUI's `dashPhase`), so growing it
    /// marches the dashes anticlockwise. A phase of `dash + gap` is one whole period. It is
    /// applied as given every frame and never springs (a spring would run a wrapped phase
    /// backwards), so drive it from a clock for marching ants.
    pub fn border_dash_phase(mut self, phase: f32) -> Self {
        let p = if phase.is_finite() { phase } else { 0.0 };
        if p != 0.0 || !self.vis.cold.is_empty() {
            self.vis.cold_mut().border_dash_phase = p;
        }
        self
    }
    /// An outer drop shadow offset by `(0, y)` with CSS `blur` radius (`blur = 2σ`; pass
    /// `2.0 * r` to match `CALayer.shadowRadius`). Springs like [`Self::box_shadow`].
    pub fn shadow(mut self, y: f32, blur: f32, color: Color) -> Self {
        self.vis.cold_mut().shadows.push(Shadow { y, blur, color, ..Shadow::default() });
        self
    }
    /// A full box shadow (offset, spread, inset), e.g. one layer of a CSS elevation token.
    /// `s.blur` follows CSS (`blur = 2σ`, twice `CALayer.shadowRadius`). The first four
    /// shadows spring: a change to one's offset, blur or spread moves on the element's layout
    /// spring and its colour on the colour spring, so a pressed card's shadow tightens as it
    /// sinks. Under Reduce Motion, and in an instant transaction, the geometry lands at once.
    pub fn box_shadow(mut self, s: Shadow) -> Self {
        self.vis.cold_mut().shadows.push(s);
        self
    }
    /// A vertical gradient from `top` to `bottom` painted over the fill (CSS `linear-gradient(180deg, ..)`).
    pub fn gradient(mut self, top: Color, bottom: Color) -> Self {
        self.vis.cold_mut().gradient = Some(Box::new([top, bottom]));
        self
    }
    /// A radial gradient painted over the fill and clipped to the element's rounded corners.
    ///
    /// `center` is in normalized element coordinates (`[0.5, 0.5]` is the centre, `[0.0, 0.0]` top-left).
    /// `radius` is in points (`f32` for a circle or `[rx, ry]` for an ellipse).
    /// `stops` are up to 16 `(position, color)` pairs along `0.0..=1.0` from `center` (`0.0`) to
    /// `radius` (`1.0`). The first 8 stops' colours spring in Oklab when changed; all interpolate
    /// in premultiplied colour across the quad, matching [`El::gradient`].
    pub fn radial_gradient(
        mut self,
        center: [f32; 2],
        radius: impl Into<RadialRadius>,
        stops: impl AsRef<[(f32, Color)]>,
    ) -> Self {
        let [rx, ry] = radius.into().0;
        let valid = center[0].is_finite()
            && center[1].is_finite()
            && rx.is_finite()
            && ry.is_finite()
            && rx > 0.0
            && ry > 0.0;
        let r = valid
            .then(|| normalize_gradient_stops(GradientGeometry::Radial { center, radius: [rx, ry] }, stops.as_ref()))
            .flatten()
            .map(Box::new);
        if r.is_some() || !self.vis.cold.is_empty() {
            self.vis.cold_mut().radial = r;
        }
        self
    }
    /// A linear gradient at `angle` radians (CSS convention: `0.0` is bottom-to-top, `PI / 2` is
    /// left-to-right, `PI` is top-to-bottom like [`El::gradient`]) with up to 16 stops, painted over
    /// the fill: enough for the eased gradients CSS writes as a stop list, such as a scrim that
    /// fades out on an easing curve (`linear-gradient(180deg, bg 0%, … 16 stops …, transparent)`).
    pub fn linear_gradient(mut self, angle: f32, stops: impl AsRef<[(f32, Color)]>) -> Self {
        let r = angle
            .is_finite()
            .then(|| normalize_gradient_stops(GradientGeometry::Linear { angle }, stops.as_ref()))
            .flatten()
            .map(Box::new);
        if r.is_some() || !self.vis.cold.is_empty() {
            self.vis.cold_mut().radial = r;
        }
        self
    }
    /// Paints `shape` (strokes and fills in this element's points, origin top-left) over the
    /// fill and under any text. It does not affect layout or hit testing.
    pub fn shape(mut self, shape: Shape) -> Self {
        let sh = (!shape.is_empty()).then(|| Rc::new(shape));
        if sh.is_some() || !self.vis.cold.is_empty() {
            self.vis.cold_mut().shape = sh;
        }
        self
    }
    /// Shows `pixels`, scaled into the element's rect and clipped to its corner radius. The image
    /// can change without the view re-running: the next frame draws its latest contents.
    /// An element that sets only its width (`w`, `w_full`) or only its height takes the other
    /// side from the image's aspect ratio, updating when the image decodes or changes shape;
    /// [`aspect_ratio`](Self::aspect_ratio) overrides it.
    pub fn pixels(mut self, pixels: &crate::pixels::Pixels) -> Self {
        let cold = self.vis.cold_mut();
        cold.pixels = Some(pixels.clone());
        cold.pixels_fit = None;
        self
    }
    /// Shows `pixels` at its own aspect ratio, centred: `Fit` letterboxes the whole image inside
    /// the rect (the corner radius rounds the image), `Fill` covers the rect and crops the overflow.
    pub fn pixels_fit(mut self, pixels: &crate::pixels::Pixels, fit: crate::SurfaceFit) -> Self {
        let cold = self.vis.cold_mut();
        cold.pixels = Some(pixels.clone());
        cold.pixels_fit = Some(fit);
        self
    }
    /// A ring `offset` points outside the border box (CSS `outline` + `outline-offset`). Width
    /// and offset snap to device pixels as [`Self::border`]'s width does.
    pub fn outline(mut self, width: f32, offset: f32, color: Color) -> Self {
        self.vis.cold_mut().outline = Some(Outline { width, offset, color });
        self
    }
    #[inline]
    pub fn opacity(mut self, o: f32) -> Self {
        self.set_opacity(o);
        self
    }
    /// [`El::opacity`] in place.
    #[inline]
    pub fn set_opacity(&mut self, o: f32) -> &mut Self {
        self.vis.opacity = o;
        self
    }
    /// Gaussian blur of the completed subtree, in points (standard deviation). Zero has no
    /// filter cost. The renderer caps sigma at 64 device pixels; bounds grow by three sigma.
    pub fn blur(mut self, sigma: f32) -> Self {
        let sigma = sigma.max(0.0);
        if sigma != 0.0 || !self.vis.cold.is_empty() {
            self.vis.cold_mut().blur = sigma;
        }
        self
    }
    /// Override the blur target's curve. Ordinary changes use the element's motion spring.
    pub fn blur_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.vis.cold_mut().blur_motion = Some(Box::new(anim.into()));
        self
    }
    /// Dim each paint leaf in the currently built subtree, independently. Unlike `opacity`,
    /// overlapping draws retain their individual coverage and no group layer is introduced. A
    /// box with a solid fill still fades as `opacity` would: its outer shadows stay outside it
    /// and its fill and gradient fade as one.
    /// Call after adding children. Useful for static disabled styling; animation and hit-test
    /// opacity still belong to `opacity` / `live_opacity`.
    pub fn dim(mut self, alpha: f32) -> Self {
        fn apply(e: &mut El, alpha: f32) {
            e.expand_memo();
            if alpha < 1.0 || !e.vis.cold.is_empty() {
                e.vis.cold_mut().paint_alpha *= alpha;
            }
            for child in &mut e.children { apply(child, alpha); }
        }
        apply(&mut self, alpha.clamp(0.0, 1.0));
        self
    }
    /// Fade the whole subtree as one composited layer, like CSS `opacity`: a label over its
    /// button's fill shows the fill as it is, not faded through, and overlapping children don't
    /// show through each other. Static, as [`Self::dim`] is: a change lands at once, and
    /// animation and hit-test opacity still belong to `opacity` / `live_opacity`. Only an
    /// element below 1 whose paint overlaps (children, or more than one paint of its own) draws
    /// offscreen; anything else just takes the alpha. Repeated calls multiply.
    pub fn layer_opacity(mut self, alpha: f32) -> Self {
        let alpha = alpha.clamp(0.0, 1.0);
        if alpha < 1.0 || !self.vis.cold.is_empty() {
            self.vis.cold_mut().layer_opacity *= alpha;
        }
        self
    }
    /// Add children's premultiplied colours while a direct child is between opacity 0 and 1.
    /// Give two overlaid states opacity `1-t` and `t` for a dissolve without a midpoint alpha dip.
    /// Put a shared background outside this scope; its own paint also participates in addition.
    /// At rest (all direct children at 0 or 1), draw normally without a dissolve layer. Overlapping
    /// opaque states therefore use source-over at rest; this API is for replacing states.
    pub fn dissolve(mut self) -> Self {
        self.vis_flags.set(VisualFlags::PLUS_LIGHTER, true);
        self
    }
    /// Rasterize this subtree once into an offscreen texture when settled and composite that
    /// texture until the subtree's draw output changes, including while an ancestor's
    /// [`live_offset`](Self::live_offset) or [`live_scale`](Self::live_scale) moves it. While a
    /// spring, transition, or effect slot inside the subtree is active, it draws live and
    /// re-caches once settled.
    pub fn cache(mut self) -> Self {
        self.vis_flags.set(VisualFlags::CACHE, true);
        self
    }
    /// Cache this subtree like [`Self::cache`], also mixing `key` into its invalidation signature.
    pub fn cache_key(mut self, key: u64) -> Self {
        self.vis_flags.set(VisualFlags::CACHE, true);
        self.vis.cold_mut().cache_key = Some(key);
        self
    }
    /// Tags this element's instances (and its descendants', until one sets its own) for a
    /// compositor: `Runtime::marks` runs parallel to `Runtime::instances`.
    pub fn mark(mut self, mark: u32) -> Self {
        if mark != 0 || !self.vis.cold.is_empty() {
            self.vis.cold_mut().mark = mark;
        }
        self
    }
    /// Overlay drawn over the fill while hovered.
    #[inline]
    pub fn hover(mut self, c: Color) -> Self {
        self.set_hover(c);
        self
    }
    /// [`El::hover`] in place.
    #[inline]
    pub fn set_hover(&mut self, c: Color) -> &mut Self {
        self.vis.set_hover(c);
        self
    }
    /// Overlay drawn over the fill while pressed.
    #[inline]
    pub fn press(mut self, c: Color) -> Self {
        self.set_press(c);
        self
    }
    /// [`El::press`] in place.
    #[inline]
    pub fn set_press(&mut self, c: Color) -> &mut Self {
        self.vis.set_press(c);
        self
    }
    /// Shrink to `scale` about the centre while a press on this element is held, as iOS tiles
    /// and buttons do (0.96 to 0.97), springing back on release or when the pointer leaves.
    /// Put it on the element with `on_press`. Layout and hit testing keep the resting size, and
    /// Reduce Motion turns it off. `1.0` (or anything not positive) removes it.
    pub fn press_scale(mut self, scale: f32) -> Self {
        let s = (scale.is_finite() && scale > 0.0 && scale != 1.0).then_some(scale);
        if s.is_some() || !self.handlers.is_empty() {
            self.handlers.press_scale = s;
        }
        self
    }
    /// Draw this element and everything in it `k` times its size about its centre, like
    /// SwiftUI's `scaleEffect`: fills, borders, text, icons and images all scale, and text
    /// rasterizes at the scaled size once it rests. Layout keeps the unscaled size, so siblings
    /// don't move; hit testing follows the scaled rect. A change springs on the element's layout
    /// spring (or [`Self::scale_motion`]); under Reduce Motion, and in an instant transaction, it
    /// lands at once. Multiplies with transitions, `live_scale` and `press_scale`. Anything not
    /// finite or below zero is `1.0`.
    pub fn scale(mut self, k: f32) -> Self {
        self.scale = if k.is_finite() && k >= 0.0 { k } else { 1.0 };
        self
    }
    /// The curve [`Self::scale`] changes on.
    pub fn scale_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.vis.cold_mut().scale_motion = Some(Box::new(anim.into()));
        self
    }
    /// Draw this element and everything in it turned `radians` clockwise about its
    /// [`origin`](Self::origin), like SwiftUI's `rotationEffect`: fills, borders, shadows,
    /// text, icons and images all turn. Layout keeps the upright box, so siblings don't move.
    /// A change springs on the element's layout spring (or [`Self::rotate_motion`]); under
    /// Reduce Motion, and in an instant transaction, it lands at once. While turned, the
    /// subtree draws into its own layer, and a native surface inside it (a video, a web view)
    /// stays upright. Hit testing follows the turned box; the reported rect uses its upright
    /// bounding box. Anything not finite is `0.0`.
    pub fn rotate(mut self, radians: f32) -> Self {
        let r = if radians.is_finite() { radians } else { 0.0 };
        if r != 0.0 || !self.vis.cold.is_empty() {
            self.vis.cold_mut().rotate = r;
        }
        self
    }
    /// The curve [`Self::rotate`] changes on.
    pub fn rotate_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.vis.cold_mut().rotate_motion = Some(Box::new(anim.into()));
        self
    }
    /// The point this element scales and rotates about, in units of its own size: `(0.5, 0.5)`
    /// (the default) is its centre, `(0.5, 1.0)` the middle of its bottom edge, which stays put
    /// while a keycap sinks. Like SwiftUI's `anchor:`. [`Self::scale`], [`Self::rotate`],
    /// `live_scale`, `press_scale` and a transition's scale all pivot here once it's set;
    /// [`Self::offset`] ignores it. Values outside 0..1 pivot outside the element; anything not
    /// finite is the centre.
    pub fn origin(mut self, x: f32, y: f32) -> Self {
        let o = (x.is_finite() && y.is_finite() && (x, y) != (0.5, 0.5)).then_some([x, y]);
        if o.is_some() || !self.vis.cold.is_empty() {
            self.vis.cold_mut().origin = o;
        }
        self
    }
    /// Run the animations of this element and everything in it at `k` times normal speed:
    /// springs, transitions (enter, exit, layout), implicit animations, hover and press
    /// fades, loops and word reveals. `0.0` freezes them exactly where they are, `0.5` is slow
    /// motion, and changing `k` carries on from the same place. Nested scales multiply, and
    /// all of them multiply with the window's [`Cx::time_scale`](crate::Cx::time_scale).
    ///
    /// Only time slows. State still updates at once: a hover or press is tracked as usual and
    /// only its fade waits. An exiting element drops its own factor and follows its ancestors
    /// still in the tree, so one under a frozen subtree stays mid-exit until that scale rises,
    /// then finishes and is removed. A frozen subtree requests no frames.
    /// Scrolling, growth stacks and `Tween` keep the window's clock: they follow input and
    /// layout, and a frozen scroll view would ignore the wheel. Anything not finite or below
    /// zero is `1.0`.
    pub fn time_scale(mut self, k: f32) -> Self {
        let k = if k.is_finite() && k >= 0.0 { k } else { 1.0 };
        if k != 1.0 || !self.cold.is_empty() {
            self.cold_mut().time_scale = k;
        }
        self
    }
    /// A scroll area's wrapper passes its clip to the viewport, beside which it draws the bar.
    fn set_clip(&mut self, clip: [f32; 4]) {
        if self.vis.clip_to_content
            && let Some(viewport) = self.children.first_mut()
        {
            viewport.vis.cold_mut().clip = Some(clip);
        } else {
            self.vis.cold_mut().clip = Some(clip);
        }
    }
    /// Clip children to this box with soft gradient edges (never a hard crop).
    #[inline]
    pub fn clip_soft(mut self, fade: f32) -> Self {
        self.set_clip_soft(fade);
        self
    }
    /// [`El::clip_soft`] in place.
    pub fn set_clip_soft(&mut self, fade: f32) -> &mut Self {
        self.set_clip([fade; 4]);
        self
    }
    /// Clip this element and its descendants to its presented rounded rectangle. Follows
    /// [`El::corners`] and [`El::continuous_corners`] when set on this element; otherwise rounds
    /// all four corners by `radius` points (capped at half the shorter edge). Filters run before
    /// this mask. Nested masks intersect without moving either rectangle's corners; complex
    /// intersections use a layer. Hit testing uses the rectangular clip bounds, including visually
    /// clipped corner areas.
    pub fn clip_rounded(mut self, radius: f32) -> Self {
        let cold = self.vis.cold_mut();
        cold.clip_corners = None;
        cold.clip_radius = Some(radius.max(0.0));
        self
    }
    /// Clip this element and its descendants to its presented rectangle with per-corner radii
    /// `[top_left, top_right, bottom_right, bottom_left]` in points, capped at half the shorter
    /// edge and following [`El::continuous_corners`]. Unlike [`El::clip_rounded`], ignores the
    /// element's paint radii. Hit testing still uses the rectangular clip bounds.
    pub fn clip_corners(mut self, radii: [f32; 4]) -> Self {
        let clamped = radii.map(|r| r.max(0.0));
        let cold = self.vis.cold_mut();
        cold.clip_corners = Some(clamped);
        cold.clip_radius = Some(clamped.into_iter().fold(0.0, f32::max));
        self
    }
    /// Punch a round hole through this element and everything it and its descendants draw: clear
    /// within `inner` points of `center` (points from the element's top-left, which may lie
    /// outside it), whole from `outer` points, and ramping linearly between, as CSS
    /// `mask-image: radial-gradient(circle at x y, transparent inner, black outer)` does. Equal
    /// radii cut an anti-aliased edge. Room for a badge over an avatar's corner:
    /// `.cutout_circle([16.0, 0.0], 4.0, 5.0)` under a 6 point dot at the corner. The subtree
    /// draws in an isolated layer, whose composite cuts the hole; hit testing ignores it.
    pub fn cutout_circle(mut self, center: [f32; 2], inner: f32, outer: f32) -> Self {
        let inner = inner.max(0.0);
        self.vis.cold_mut().cutout = Some([center[0], center[1], inner, outer.max(inner)]);
        self
    }
    /// Soft clip with a fade width per side (left, top, right, bottom).
    pub fn clip_fade(mut self, left: f32, top: f32, right: f32, bottom: f32) -> Self {
        self.set_clip([left, top, right, bottom].map(|w| w.max(0.0)));
        self
    }
    /// [`Self::clip_fade`] with linear ramps: alpha falls in proportion to distance, from whole
    /// `width` in from each side to nothing at it, rather than easing at both ends. A mask that
    /// runs straight into an edge, such as a transcript fading into the top of a composer.
    pub fn clip_fade_linear(mut self, left: f32, top: f32, right: f32, bottom: f32) -> Self {
        self.set_clip([left, top, right, bottom].map(|w| -w.max(0.0)));
        self
    }
    /// Track hover for descendants' `group_*` styles (see [`Visual::hover_group`]).
    pub fn hover_group(mut self) -> Self {
        self.vis_flags.set(VisualFlags::HOVER_GROUP, true);
        self
    }
    /// [`Self::hover_group`], shared by every element on screen with the same `key`: the pointer
    /// over any of them hovers them all, so one group can span elements that don't nest, such as
    /// a reply's text and the footer under it in separate rows of a virtual list.
    pub fn hover_group_shared(mut self, key: impl Into<Rc<str>>) -> Self {
        self.vis_flags.set(VisualFlags::HOVER_GROUP, true);
        self.vis.cold_mut().hover_share = Some(key.into());
        self
    }
    /// [`Self::hover_group`] that also counts as hovered while keyboard focus is on it or
    /// anywhere inside it, as CSS `:is(:hover, :has(:focus-visible))` does: a tab whose close
    /// button and hover wash show when Tab reaches it, as under the pointer.
    pub fn hover_group_focus(mut self) -> Self {
        self.vis_flags.set(VisualFlags::HOVER_GROUP, true);
        self.vis.cold_mut().hover_focus = true;
        self
    }
    /// Curves for this element's hover track: `enter` when the pointer arrives, `exit` when it
    /// leaves (e.g. a delayed reveal).
    pub fn hover_motion(mut self, enter: impl Into<Anim>, exit: impl Into<Anim>) -> Self {
        self.vis.cold_mut().hover_motion = Some(Rc::new((enter.into(), exit.into())));
        self
    }
    /// Default hover curves for this element and everything below it: any hover track (an
    /// element with [`Self::hover`] or [`Self::hover_group`]) without its own
    /// [`Self::hover_motion`] uses the nearest ancestor's pair, and [`Self::anchored`] overlays
    /// inherit from their anchor, so kit controls whose inner elements you can't reach follow it
    /// too. Set it on the root of the view (or on what you pass to `rune_kit::root`, which
    /// carries it to the overlays it layers beside the view). Without any, hover lands at once
    /// and fades out over 0.1 s. Rows highlighted by `focus_highlight` (menu items, select
    /// options) are never faded.
    pub fn hover_motion_within(mut self, enter: impl Into<Anim>, exit: impl Into<Anim>) -> Self {
        self.vis.cold_mut().hover_within = Some(Rc::new((enter.into(), exit.into())));
        self
    }
    /// The pair set by [`Self::hover_motion_within`].
    pub fn hover_within_curves(&self) -> Option<(Anim, Anim)> {
        self.vis.hover_within.as_deref().copied()
    }
    /// Opacity factor `rest` normally and `hovered` while the nearest ancestor group is hovered.
    pub fn group_opacity(mut self, rest: f32, hovered: f32) -> Self {
        self.vis.cold_mut().group_opacity = Some([rest, hovered]);
        self
    }
    /// Text or icon colour while the nearest ancestor group is hovered.
    pub fn group_color(mut self, c: Color) -> Self {
        self.vis.cold_mut().group_color = Some(c);
        self
    }
    /// Hovering this element shows only its own hover: while the pointer is on it, the
    /// elements it sits inside show no hover overlay and their `group_*` styles rest, as if the
    /// pointer had left them. A face or button inside a hoverable row, for one. Paint only: it
    /// changes no layout and needs no commit.
    pub fn hover_alone(mut self) -> Self {
        self.vis.cold_mut().hover_alone = true;
        self
    }
    /// Wall-clock opacity loop, using the shared loop engine and host wake deadline.
    pub fn cycle(self, c: Cycle) -> Self {
        self.looping(rune_motion::LoopSpec::cycle(c))
    }
}
