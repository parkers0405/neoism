//! Platform-neutral contract for experimental inline HTML artifacts.
//! The real native Servo engine is available only with the `servo` feature.
#![forbid(unsafe_code)]

pub use keyboard_types::{
    Code, CompositionEvent, CompositionState, Key, KeyState, KeyboardEvent, Location,
    Modifiers, NamedKey,
};
use std::sync::Arc;

pub mod theme;
pub use theme::{ArtifactColors, ArtifactStyles, StyleError};

#[cfg(all(feature = "servo", not(target_arch = "wasm32")))]
pub mod servo_host;

#[cfg(all(feature = "ipc", not(target_arch = "wasm32")))]
pub mod ipc;

pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_VIEWS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidViewport,
    InvalidStyles(StyleError),
    HtmlTooLarge,
    InvalidKey,
    TooManyViews,
    RevisionConflict,
    UnknownArtifact,
    InvalidInput,
    EngineAlreadyInitialized,
    Backend(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

/// Width and height in physical pixels; scale is physical pixels per CSS pixel.
#[cfg_attr(feature = "ipc", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}
impl Viewport {
    pub fn validate(self) -> Result<(), Error> {
        if self.width == 0
            || self.height == 0
            || self.width > 8192
            || self.height > 8192
            || !self.scale.is_finite()
            || !(0.25..=8.0).contains(&self.scale)
        {
            return Err(Error::InvalidViewport);
        }
        let bytes = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|v| v.checked_mul(4))
            .ok_or(Error::InvalidViewport)?;
        if bytes > MAX_FRAME_BYTES {
            return Err(Error::InvalidViewport);
        }
        Ok(())
    }
}

#[cfg_attr(feature = "ipc", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

#[cfg_attr(feature = "ipc", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct ArtifactDocument {
    pub key: String,
    pub html: String,
    /// Change this when replacing the document. Same revision + different HTML is an error.
    pub revision: u64,
    pub viewport: Viewport,
    pub visible: bool,
    pub theme: Theme,
    /// Live renderer tokens; updates must not reload the source document.
    pub styles: ArtifactStyles,
}
impl ArtifactDocument {
    pub fn validate(&self) -> Result<(), Error> {
        self.viewport.validate()?;
        self.styles.validate().map_err(Error::InvalidStyles)?;
        if self.key.is_empty() || self.key.len() > 1024 {
            return Err(Error::InvalidKey);
        }
        if self.html.len() > MAX_HTML_BYTES {
            return Err(Error::HtmlTooLarge);
        }
        Ok(())
    }
}

#[cfg_attr(feature = "ipc", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Middle,
    Right,
    Back,
    Forward,
}
#[cfg_attr(feature = "ipc", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonState {
    Down,
    Up,
}
#[cfg_attr(feature = "ipc", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelUnit {
    Pixel,
    Line,
    Page,
}

#[cfg_attr(feature = "ipc", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub enum ArtifactInput {
    PointerMove {
        x: f32,
        y: f32,
    },
    PointerButton {
        x: f32,
        y: f32,
        button: PointerButton,
        state: ButtonState,
    },
    PointerLeave,
    /// Deltas follow DOM wheel convention: positive means scrolling right/down.
    Wheel {
        x: f32,
        y: f32,
        delta_x: f64,
        delta_y: f64,
        unit: WheelUnit,
    },
    Key(KeyboardEvent),
    /// Forward native composition start/update/end; do not also send commit as a key.
    Ime(CompositionEvent),
    ImeDismissed,
    Focus(bool),
}
impl ArtifactInput {
    pub fn validate(&self) -> Result<(), Error> {
        let point = match self {
            Self::PointerMove { x, y } | Self::PointerButton { x, y, .. } => {
                Some((*x, *y))
            }
            Self::Wheel {
                x,
                y,
                delta_x,
                delta_y,
                ..
            } => {
                if !delta_x.is_finite() || !delta_y.is_finite() {
                    return Err(Error::InvalidInput);
                }
                Some((*x, *y))
            }
            Self::Key(event) => {
                if let Key::Character(text) = &event.key {
                    if text.len() > 64 * 1024 {
                        return Err(Error::InvalidInput);
                    }
                }
                None
            }
            Self::Ime(event) => {
                if event.data.len() > 64 * 1024 {
                    return Err(Error::InvalidInput);
                }
                None
            }
            _ => None,
        };
        if point.is_some_and(|(x, y)| !x.is_finite() || !y.is_finite()) {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}

/// Immutable owned top-down RGBA8 snapshot; rows are tightly packed.
/// WebRender output uses premultiplied alpha. No native GPU handles cross this API.
#[cfg_attr(feature = "ipc", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct ArtifactFrame {
    pub key: String,
    pub revision: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub rgba: Arc<[u8]>,
}
#[derive(Default, Debug)]
pub struct PumpOutput {
    pub frames: Vec<ArtifactFrame>,
    pub animating: bool,
    pub diagnostics: Vec<String>,
}

/// Deliberately not called "sandboxed": application policy cannot contain engine exploits.
#[cfg_attr(feature = "ipc", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SandboxStatus {
    pub os_sandbox: bool,
    pub in_process: bool,
    pub resource_policy: &'static str,
    pub limitations: &'static str,
}
pub const SANDBOX_STATUS: SandboxStatus = SandboxStatus {
    os_sandbox: false,
    in_process: false,
    resource_policy: "Exact synthetic document GET only; all other intercepted loads cancelled; navigation, permissions, clipboard and popups denied; restrictive CSP",
    limitations: "Experimental trusted-content only. Separate process, but no OS sandbox, memory/CPU quotas or audited engine-exploit containment. CSP and interception coverage are not an audited security boundary.",
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn viewport_bounds() {
        let valid = Viewport {
            width: 640,
            height: 320,
            scale: 2.0,
        };
        assert!(valid.validate().is_ok());
        for invalid in [
            Viewport { width: 0, ..valid },
            Viewport {
                scale: f32::NAN,
                ..valid
            },
            Viewport {
                height: 8192,
                width: 8192,
                ..valid
            },
            Viewport {
                scale: 0.0,
                ..valid
            },
        ] {
            assert_eq!(invalid.validate(), Err(Error::InvalidViewport));
        }
    }
    #[test]
    fn reject_nonfinite_input() {
        assert_eq!(
            ArtifactInput::PointerMove {
                x: f32::INFINITY,
                y: 0.0
            }
            .validate(),
            Err(Error::InvalidInput)
        );
        assert_eq!(
            ArtifactInput::Wheel {
                x: 0.0,
                y: 0.0,
                delta_x: f64::NAN,
                delta_y: 0.0,
                unit: WheelUnit::Pixel
            }
            .validate(),
            Err(Error::InvalidInput)
        );
    }
    #[test]
    fn status_never_claims_sandbox() {
        assert!(!SANDBOX_STATUS.os_sandbox);
        assert!(!SANDBOX_STATUS.in_process);
    }
    #[test]
    fn snapshot_survives_producer() {
        let bytes: Arc<[u8]> = vec![1, 2, 3, 4].into();
        let frame = ArtifactFrame {
            key: "a".into(),
            revision: 1,
            sequence: 1,
            width: 1,
            height: 1,
            stride: 4,
            rgba: bytes.clone(),
        };
        drop(bytes);
        assert_eq!(&*frame.rgba, &[1, 2, 3, 4]);
    }
}
