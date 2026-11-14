# Native Liquid Glass cursor

## Active rail selection (ordered under-content material)

`appearance.effects.liquid-glass-selection` is a separate opt-in boolean, default **false**. It does not follow `liquid-glass-cursor`. Rune loads it at startup, applies config replies and Settings writes live, and supplies a descriptor for older daemons. No personal config is changed by installing the implementation.

The selection uses the existing Rune `El::effect(mark).effect_backdrop(true)` / `Renderer::draw_with` compositor API, not the cursor's pre-present callback. `src/glass_selection.rs` attaches one keyed 40-point slot to the rail. `src/ui/chrome.rs` paints rail background, paint-only shared hover washes, the moving optical slot, then the original buttons/icons. The hover washes share the original button's hover group and retain Rune's hover color/timing. There are no copied glyphs or duplicate navigation handlers. The slot follows the active panel/menu/finder/Systems state, not keyboard focus.

`src/glass_selection.wgsl` samples the live **earlier** painter content, with modest magnification, curved shoulder refraction, restrained channel separation and a neutral rim. Rune's native Metal backend compiles/encodes it on the existing GPU; production does not take screenshots, map pixels or read back the framebuffer. This is a custom optical material, not an `NSGlassEffectView` and not the Swift cursor's final pass. A non-bouncy 0.42-second offset spring moves the one pill; velocity produces at most 5 points of vertical stretch, with no time-driven shimmer or animation at rest. Black themes keep a near-black face instead of a gray frosted sheet.

Reduce Motion snaps placement and disables stretch. Reduce Transparency or Increase Contrast selects the solid, outlined Rune fallback. Other platforms, explicit `RUNE_GPU_BACKEND=wgpu`, and `--snap`/`--tree` also use the fallback. Accessibility policy is queried only on the UI thread; an unchanged periodic policy check does not request a frame. GPU resources are keyed by Rune `DeviceKey` and render format; the renderer owns the pooled backdrop and command lifetimes.

For another under-content optical surface, reuse Rune's ordered effect-slot API and add its uniquely marked handler to the existing app compositor; do not install a second app compositor or use the cursor's pre-present pass. Draw its foreground after the slot, keep hit targets on the original controls, and keep resource/uniform storage separate per visible slot. The current rail handler intentionally supports only its one mark. Rune currently copies a viewport-sized pooled GPU backdrop at this API boundary; it is not a CPU/readback path, but large or numerous new surfaces need copy-bandwidth profiling before reusing it broadly.

## Cursor activation

The system mouse pointer is separate from the app's keyboard caret. An app-local AppKit event monitor hides it on key-down using `NSCursor.setHiddenUntilMouseMoves(true)`, including navigation inside text fields, menus and pickers. AppKit restores it on mouse movement; mouse-button presses and window key-focus loss explicitly clear the hidden-until-move state. This policy is independent of the Liquid Glass toggle. Modifier-only changes do not hide it, and the monitor neither consumes events nor reads or records their key/text contents.

Enable the opt-in app-wide keyboard cursor material in `~/.config/neoism/config.json` (or Settings > Appearance):

```json
{
  "appearance": {
    "effects": {
      "liquid-glass-cursor": true
    }
  }
}
```

`GlassCursor.swift` receives the exact live quad from Rust's original `ui::cursor::Trail`: four independent corner springs, with faster leading corners and slower trailing corners. Rust owns animation, reduced-motion snapping, blink and immediate scroll transport for both materials; Swift does not apply a second spring or velocity-based rectangle stretch. `GlassOptics.swift` rounds the actual quad corners and uses its signed edge distance and normals for the silhouette, shoulder refraction, RGB dispersion and reflections, including diagonal and turning shapes. The face magnifies approximately 1.22× around the quad's center. It renders into the actual Rune drawable before that drawable is presented. Text updates and scrolling are sampled from the current Rune frame, not an image captured on drag or target change.

There is no overlay NSView, so no hit testing, focus, accessibility element, safe-area offset or second presentation clock. Logical Rune coordinates are multiplied by the drawable-to-layer scale. A Metal scissor contains the lens, including highlights and deformation. Native readiness is acknowledged before removing the fallback caret. Blink, visibility and clip changes request frames while the spring rests; animation uses Rune's existing vsync heartbeat.

Renderer ownership is independent of target placement: a tab/workspace whose incoming layout has no caret yet hides the lens without selecting the ordinary material. The ordinary quad's live opacity also obeys ownership and placement, so an already-mounted quad cannot relight during a layout gap or native handoff. Native readiness/failure notifications survive intervening encoded frames until the UI acknowledges them; frame dirtiness alone is not a readiness notification. Unsupported backends, accessibility restrictions and genuinely occluding popups still restore the ordinary cursor immediately.

During larger movements, the sides parallel to travel develop a restrained inward waist, biased toward the trailing end so the leading rounded end stays fuller. Curvature fades in smoothly from 320 to 1,360 logical points/second using the existing velocity uniform; it adds no animation state, translation spring or scroll lag. The inward bend is based on 12% of the shortest quad edge, capped at 3 logical points (with a small asymmetric envelope bias). Twelve bounded segments per side approximate a smooth quartic envelope with zero endpoint slope; the same nearest-boundary calculation supplies silhouette, optical normals, prism and reflections. The original corner curves, 12% enlargement, magnifying center and adaptive rim are retained. At or below 320 points/second the original straight-side path is used exactly. All curved samples remain inside the existing copy bounds; this is a single connected lens, not a metaball trail or extra smear.

Reduce Motion snaps the cursor to its destination. Reduce Transparency or Increase Contrast uses the normal Rune cursor. Other operating systems, explicitly selected `RUNE_GPU_BACKEND=wgpu`, and offscreen/headless rendering also retain the normal cursor. Settings changes apply without restarting. This is a custom optical approximation, not Apple's private Liquid Glass implementation or a claim of exact iPhone parity. Thin insertion beams naturally have less visible refraction than block carets.

A final native pass cannot occupy a position between Rune's GPU draw calls. While a popup needs to occlude an underlying cursor (completion, hover, context menu, modal, drag preview), the renderer falls back to Rune's cursor to preserve ordering. Stored editor hover/completion state does not count while chrome owns focus, because the editor popup is not rendered then. The lens can still mark controls inside finder, palette, menus and settings, and composer/query carets beside slash suggestions. The Swift target state follows the existing single-main-window bridge; a future multi-window-in-one-process app should make that state per view too.

## GPU path and safety

The default Rune backend on macOS is Rune's native Metal implementation, not wgpu-hal. The minimal per-view `rune::native_frame` hook borrows the live `MTLCommandBuffer` and `MTLTexture`. Swift uses that same device and command buffer. There is **zero-copy interop and no CPU pixel transfer**, but not literally zero GPU copying: a small lens-bounding rectangle is blitted into a pooled private texture each drawn frame to avoid illegal sampling/render-target feedback. No full-window intermediate, IOSurface, cross-queue fence, independent display link, or screenshot API is needed. The existing execution order provides synchronization. A final event signal includes the effect in Rune's frame pacing.

Backdrop capacity grows in 64-pixel buckets as needed. Same-queue accesses serialize reuse. Completion handlers explicitly retain the backdrop, drawable texture and pipeline because Rune's command buffers have unretained references. Handlers never capture the command buffer. Resize, disable and window replacement cannot release in-flight resources. Target state and rendering state share a lock; AppKit accessibility preferences are queried only on the main thread. The shader/pipeline is compiled lazily on first opt-in use, and failure restores the ordinary cursor.

## Build and verification

`build.rs` compiles `GlassCursor.swift` and `GlassOptics.swift` into a static archive for the Cargo target architecture using the installed Xcode Swift compiler. The macOS deployment target remains 13.0; no macOS 26 glass API is used. Linux and Windows builds do not invoke Swift.

```sh
cd rune/app
./build.sh build
./build.sh test cursor -- --nocapture
xcrun swiftc native/GlassCursor.swift native/GlassOptics.swift native/GlassCursorMotionTests.swift -o /tmp/neoism-glass-motion-tests
/tmp/neoism-glass-motion-tests
xcrun swiftc native/GlassOptics.swift native/GlassOpticsTests.swift -o /tmp/neoism-glass-tests
MTL_DEBUG_LAYER=1 /tmp/neoism-glass-tests
```

The standalone GPU test verifies current-frame source replacement, magnification, RGB dispersion along deformed edges, asymmetric/turning silhouettes, velocity response, scissoring, offscreen/beam geometry, scale factors, asynchronous resizing and explicit lifetime retention with Metal API validation. CPU readback is confined to that test executable, never the production renderer. Rust cursor tests cover independent corner dynamics, exact settling, fractional scroll transport, and repeated real tab/workspace selections with missing incoming layout and a retained ordinary-opacity binding. Swift motion tests verify exact quad delivery without a second animation, settled-frame invalidation, reappearance and reenable; they also exercise the exported native bridge with actual Metal command buffers, including multiple frames before readiness acknowledgement, absent targets and policy disable/reenable (skipped when system accessibility policy forbids glass).

Droplet regression checks also compile a straight-side reference shader with only the fluid amplitude disabled: settled and slow rendering must match byte-for-byte, while fast motion must change the coverage mask, retain a connected centerline, preserve the central test glyph and disperse RGB at the curved shoulder. Reversing travel must reverse the silhouette bias, not merely the lighting. The bounded subdivision increases moving-fragment work; no frame-pacing parity is implied by these correctness tests.

Use `--snap` for layout/fallback checks; verify native optics in a live window with an isolated enabled config and `NEOISM_RUNE_OFFLINE=1`. For example, set `NEOISM_CONFIG_HOME=/tmp/neoism-glass-check` and `NEOISM_RUNE_DRIVE='wait:2;keys:^:2~:20;wait:6;keys:iX^:10;keys:^:2~:20;wait:6;quit'` with `--scene lsp`. `^` dismisses the initial hover and `:2~` moves to the first nonblank glyph on line 2. Use `--scene editor-long` and `touch:-300:3;touch:300:3` for scrolling. Use new movie output paths for each `screencapture`. Frame-rate claims require presentation timestamps, not capture frame counts or the FPS badge.

## Motion references

For focus-zone regression checks, the drive supports `alt:up`, `alt:down`, `alt:left` and `alt:right` through the real keyboard dispatcher, logging focus, target and native-material readiness. Run an isolated offline `--scene editor-long` window with `NEOISM_RUNE_DRIVE='wait:2;alt:left;wait:1;alt:right;wait:1;alt:up;wait:1;alt:up;wait:1;alt:down;wait:1;alt:left;wait:1;alt:down;wait:1;quit'` with the effect enabled and disabled. Alt+Arrow remains app-wide on macOS as well as other platforms; Ctrl+Cmd+Arrow is an additional macOS alias. Remote `GetConfig` can override the local material preference, but never changes this navigation ladder. Do not alter remote configuration to diagnose keyboard dispatch.

- [LiquidGlassTabBar](https://github.com/IchimTeodor/LiquidGlassTabBar): its README and optical shader informed the comparison of snapshot/presentation strategies. Its static drag snapshot is unsuitable for a text editor. No code or shader source from that project is incorporated; its root LICENSE URL was unavailable when inspected. The shader here is original and uses only public APIs.

- [Apple: Applying Liquid Glass to custom views](https://developer.apple.com/documentation/swiftui/applying-liquid-glass-to-custom-views): persistent containers, glass identities, and animated state changes.
- [Apple HIG: Materials](https://developer.apple.com/design/human-interface-guidelines/materials) and [Color](https://developer.apple.com/design/human-interface-guidelines/color#Liquid-Glass-color): clear glass prioritizes the visible detail of underlying content; glass has no inherent color. This custom cursor deliberately acts as a content lens rather than a frosted toolbar control.
- [Apple: NSHostingView.safeAreaRegions](https://developer.apple.com/documentation/swiftui/nshostingview/safearearegions): the default is `.all`; disable it when integrating an already window-positioned overlay.
- [GetStream: awesome-liquid-glass](https://github.com/GetStream/awesome-liquid-glass): native slider and tab-bar squash/stretch examples. These use system controls; `.interactive()` alone cannot synthesize their gestures for a click-through keyboard cursor.
- [BleepingSwift: Containers, Morphing and Buttons](https://bleepingswift.com/blog/liquid-glass-swiftui-introduction): identity/container setup and why state changes without animation do not morph.