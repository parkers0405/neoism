//! Rune, first slice: keyed element tree from plain view functions, Taffy layout, analytic
//! spring tracks, an instanced wgpu uber-quad renderer with Parley text, pointer and keyboard
//! input, scroll viewports, growth stacks, streaming reveals, themes as data, and headless agent
//! tooling.

#[cfg(all(target_arch = "wasm32", any(feature = "hot", feature = "native-file-dialog")))]
compile_error!("web does not support Rune's hot or native-file-dialog features; disable them for wasm32");

mod a11y;
mod app;
mod cold;
mod cursor;
mod desktop;
mod flight;
mod frame;
mod gpu;
#[cfg(target_os = "macos")]
pub mod native_frame;
mod input;
mod latest;
mod native_menu;
mod open_url;
mod overlay;
mod pixels;
mod platform;
mod popup_menu;
mod quit;
mod recorder;
mod shortcuts;
mod states;
mod system_material;
mod titlebar;
mod video;
mod window;

pub mod clipboard;
pub mod color;
pub mod compose;
pub mod dirs;
pub mod drag_out;
pub mod edit;
pub mod el;
pub mod file_dialog;
pub mod growth;
#[cfg(feature = "hot")]
pub mod hot;
pub mod image;
pub mod labels;
pub mod live;
pub mod machine;
pub mod memo;
pub mod motion_gate;
pub mod native_surface;
pub mod playground;
pub mod prefs;
pub mod render;
pub mod rewind;
pub mod runtime;
pub mod shape;
pub mod spell;
pub mod text;
pub mod theme;
pub mod time;
pub mod tok;
pub mod tooling;
pub mod tray;
pub mod windows;

/// AccessKit's types, for [`Runtime::accessibility_tree`] and [`Runtime::accessibility_action`].
pub use a11y::AccessibilityLink;
pub use accesskit;
pub use app::App;
pub use app::AppHandle;
pub use clipboard::ClipboardContent;
pub use clipboard::ClipboardImage;
pub use clipboard::ClipboardWrite;
pub use clipboard::SecretCopy;
pub use clipboard::SecretState;
pub use color::Color;
pub use color::oklch;
pub use color::oklcha;
pub use compose::Compositor;
pub use compose::Frame;
pub use drag_out::DragImage;
pub use drag_out::DragItem;
pub use drag_out::DragOperation;
pub use drag_out::DragOutcome;
pub use edit::Block;
pub use edit::Ime;
pub use edit::Key;
pub use edit::KeyEvent;
pub use edit::KeySide;
pub use edit::MAX_LIST_LEVEL;
pub use edit::Mark;
pub use edit::MarkRange;
pub use edit::Modifier;
pub use edit::Mods;
pub use edit::ParagraphInfo;
pub use edit::RichStyle;
pub use edit::TextEdit;
pub use el::Align;
pub use el::Anchor;
pub use el::Cursor;
pub use el::CursorImage;
pub use el::Cycle;
pub use el::Direction;
pub use el::Drag;
pub use el::El;
pub use el::GrowthSpec;
pub use el::InlineIcon;
pub use el::LiveRegion;
pub use el::Outline;
pub use el::Phase;
pub use el::PillStyle;
pub use el::PointerEvent;
pub use el::PointerPress;
pub use el::RadialRadius;
pub use el::ScrollFade;
pub use el::ScrollSpec;
pub use el::SelectionStyle;
pub use el::Shadow;
pub use el::Shell;
pub use el::Side;
pub use el::Tx;
pub use el::anchor_rect;
pub use el::auto_grow;
pub use el::canvas;
pub use el::col;
pub use el::edit_access;
pub use el::effect;
pub use el::el;
pub use el::icon;
pub use el::layout_watch::ScrollPos;
pub use el::motion::MotionIntent;
pub use el::rich;
pub use el::row;
pub use el::spacer;
pub use el::text;
pub use el::text_input;
pub use file_dialog::FileDialogError;
pub use file_dialog::FileDialogKind;
pub use file_dialog::FileDialogOptions;
pub use file_dialog::FileDialogResult;
pub use file_dialog::FileDialogs;
pub use file_dialog::ScriptedFileDialog;
pub use frame::encoded::FrameEncoded;
pub use frame::present_time::PresentationFeedback;
pub use frame::encoded::FrameOutcome;
#[cfg(feature = "frame-profile")]
#[doc(hidden)]
pub use frame::profile::PHASES;
#[cfg(feature = "frame-profile")]
#[doc(hidden)]
pub use frame::profile::take_frame_phases;
#[cfg(feature = "frame-profile")]
#[doc(hidden)]
pub use frame::profile::thread_cpu_ms;
pub use frame::surface_stats::Pacing;
pub use frame::surface_stats::SurfaceCounts;
pub use frame::surface_stats::SurfaceStats;
pub use gpu::backend::GpuBackend;
pub use gpu::backend::describe as describe_gpu;
#[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
#[doc(hidden)]
pub use gpu::metal::bench::dispatch_overhead as __metal_dispatch_overhead;
pub use gpu::springs as gpu_springs;
pub use growth::GROWTH;
pub use growth::StreamEdge;
pub use growth::edge;
pub use image::Image;
pub use image::ImageCache;
pub use image::ImageError;
pub use image::ImageId;
pub use input::canvas_input::GesturePhase;
pub use input::canvas_input::MagnifyEvent;
pub use input::canvas_input::PointerButton;
pub use input::canvas_input::RotateEvent;
pub use input::canvas_input::WheelEvent;
pub use input::chord::Chord;
pub use input::chord::ChordError;
pub use input::chord::ChordKey;
pub use input::chord::ChordMod;
pub use input::chord::KeyPlatform;
pub use input::chord::Keybind;
pub use input::physical_input::PhysicalInput;
pub use input::time::last_input;
pub use labels::Label;
pub use labels::set_labels;
pub use latest::Latest;
pub use live::Live;
pub use live::LiveText;
pub use live::Tween;
pub use memo::memo;
pub use native_menu::MenuError;
pub use native_menu::MenuItem;
pub use native_surface::NativeSurface;
pub use native_surface::SurfaceFit;
pub use open_url::OpenUrlError;
pub use open_url::instance::single_instance;
pub use overlay::Availability;
pub use overlay::OverlayHandle;
pub use overlay::OverlaySpec;
pub use overlay::ScreenEdge;
pub use overlay::options::ConfiguredOverlay;
pub use overlay::options::DisplayId;
pub use overlay::options::DisplayTarget;
pub use overlay::options::KeyPolicy;
pub use overlay::options::NativeDisplayId;
pub use overlay::options::OverlayCapabilities;
pub use overlay::options::OverlayCreated;
pub use overlay::options::OverlayInput;
pub use overlay::options::OverlayLevel;
pub use overlay::options::OverlayOptions;
pub use overlay::options::OverlayPlacement;
pub use overlay::options::OverlaySupport;
pub use pixels::Pixels;
pub use platform::accent::SystemAccent;
pub use platform::accent::system_accent;
pub use platform::app_icon::set_app_icon;
#[cfg(target_vendor = "apple")]
pub use platform::embed;
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub use platform::macos::appearance as macos_appearance;
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub use platform::macos::panel_bridge as macos_panel_bridge;
pub use platform::scroll_bars::ScrollBars;
#[doc(hidden)]
pub use platform::scroll_bars::current_runtime_id;
pub use platform::scroll_bars::system_scroll_bars;
pub use platform::scroll_bars::system_scroller_jumps;
pub use platform::scroll_bars::system_scroller_overlay;
#[cfg(target_arch = "wasm32")]
pub use platform::web;
pub use playground::Play;
pub use playground::Playground;
pub use playground::Stage;
pub use playground::Step;
pub use playground::Story;
pub use popup_menu::PopupAnchor;
pub use popup_menu::PopupItem;
pub use popup_menu::PopupMenu;
pub use popup_menu::PopupMenuError;
pub use popup_menu::popup_menu_supported;
pub use quit::quitting;
pub use recorder::FakeMicrophone;
pub use recorder::Level;
pub use recorder::MicPermission;
pub use recorder::RecordFormat;
pub use recorder::Recorder;
pub use recorder::RecorderState;
pub use recorder::Recording;
pub use render::MAX_BACKDROP_BLURS;
pub use render::gpu_events::GpuEvent;
pub use render::gpu_events::on_gpu;
pub use render::prewarm_gpu;
pub use render::prewarm_gpu_on;
pub use render::warm_gpu;
pub use render::warm_gpu_with;
pub use rune_motion::Anim;
pub use rune_motion::AnimExt;
pub use rune_motion::Beat;
pub use rune_motion::Blend;
pub use rune_motion::Clock;
pub use rune_motion::CubicBezier;
pub use rune_motion::Curve;
pub use rune_motion::Decay;
pub use rune_motion::Easing;
pub use rune_motion::Edge;
pub use rune_motion::Effect;
pub use rune_motion::LayoutAnim;
pub use rune_motion::LoopKind;
pub use rune_motion::LoopSpec;
pub use rune_motion::NodeId;
pub use rune_motion::Phase as PresencePhase;
pub use rune_motion::Presented;
pub use rune_motion::Reveal;
pub use rune_motion::Sequence;
pub use rune_motion::Spring;
pub use rune_motion::SpringPair;
pub use rune_motion::Stagger;
pub use rune_motion::StaggerFrom;
pub use rune_motion::StepPosition;
pub use rune_motion::Transition;
pub use rune_motion::Travel;
pub use rune_motion::Vec2;
pub use rune_motion::Velocity;
pub use runtime::Cx;
pub use runtime::ElementInfo;
pub use runtime::Env;
pub use runtime::HelpLook;
pub use runtime::MotionElement;
pub use runtime::Pick;
pub use runtime::Picked;
pub use runtime::Pose;
pub use runtime::Runtime;
pub use runtime::SelectedText;
pub use runtime::SelectionChange;
pub use runtime::TextSelection;
pub use runtime::TitlebarAction;
pub use runtime::TouchPhase;
pub use runtime::Waker;
pub use runtime::commit_generation;
#[doc(hidden)]
pub use runtime::scroll_watch_state;
pub use shape::LineCap;
pub use shape::Path;
pub use shape::Shape;
pub use shortcuts::PhysicalKeyCode;
pub use shortcuts::Shortcut;
pub use shortcuts::ShortcutError;
pub use shortcuts::ShortcutId;
pub use shortcuts::modifier_keycaps;
pub use system_material::MAX_NATIVE_MATERIALS;
pub use system_material::SystemMaterial;
pub use text::FontSmoothing;
pub use text::Icon;
pub use text::Link;
pub use text::LinkPaint;
pub use text::Span;
pub use text::TextCase;
pub use text::TextHighlight;
pub use theme::Theme;
pub use time::LocalTime;
pub use titlebar::ChromeInsets;
pub use titlebar::Titlebar;
pub use tok::Family;
pub use tok::OpticalSize;
pub use tok::TextStyle;
pub use tooling::AppSpec;
pub use tooling::Chrome;
pub use tooling::extra_args;
pub use tooling::is_headless;
pub use tooling::run;
pub use tooling::run_windows;
pub use tooling::run_windows_with;
pub use tooling::run_windows_with_args;
pub use tooling::run_with;
pub use tooling::set_app_id;
pub use tooling::try_run_windows_with;
pub use tooling::try_run_windows_with_args;
pub use tray::Tray;
pub use tray::TrayError;
pub use tray::TrayEvent;
pub use tray::TrayIcon;
pub use tray::TrayItem;
pub use tray::TraySpec;
pub use video::MediaRequest;
pub use video::MediaSource;
pub use video::Player;
pub use video::PlayerState;
pub use video::Video;
pub use window::WindowHandle;
pub use windows::AnyWindowSpec;
pub use windows::ConfiguredWindowOptions;
pub use windows::Display;
pub use windows::MacWindowOptions;
pub use windows::RunOptions;
pub use windows::WindowOptions;
pub use windows::WindowPlacement;
pub use windows::WindowSpec;
pub use windows::open_window;
pub use winit::raw_window_handle;

/// The logical clock a scene fn (`AppSpec::scenes`) builds its state against. `--scene` starts
/// at time zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SceneClock {
    pub now: f64,
    pub dt: f64,
}

thread_local! {
    static SCENE_CLOCK: std::cell::Cell<SceneClock> = const { std::cell::Cell::new(SceneClock { now: 0.0, dt: 0.0 }) };
}

/// Readable from scene fns, `AppSpec::init` and `AppSpec::setup`; zero anywhere else.
pub fn scene_clock() -> SceneClock {
    SCENE_CLOCK.get()
}

/// Runs `f` (a scene fn or `init`) with `clock` as the scene clock.
pub(crate) fn with_scene_clock<T>(clock: SceneClock, f: impl FnOnce() -> T) -> T {
    let before = SCENE_CLOCK.replace(clock);
    let out = f();
    SCENE_CLOCK.set(before);
    out
}
