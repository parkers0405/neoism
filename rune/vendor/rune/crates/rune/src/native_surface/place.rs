//! Where a frame sits inside its element: which edge a spare or cropped side is shared from, and
//! whether the frame is scaled at all.
use super::SurfaceFit;
use crate::el::Align;

/// `El::surface_anchor` and `El::surface_unscaled`. The default centres and scales by `SurfaceFit`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SurfacePlace {
    pub anchor: [Align; 2],
    pub unscaled: bool,
}

impl SurfacePlace {
    pub const CENTRED: SurfacePlace = SurfacePlace { anchor: [Align::Center; 2], unscaled: false };
}

impl Default for SurfacePlace {
    fn default() -> Self { Self::CENTRED }
}

impl SurfacePlace {
    /// How much of the spare (or cropped) space along each axis lies before the image.
    pub fn share(self) -> [f32; 2] {
        self.anchor.map(|a| match a {
            Align::Start => 0.0,
            Align::Center => 0.5,
            Align::End => 1.0,
        })
    }
}

/// The image's rect in device pixels inside `node` ([x, y, w, h]); it may overflow the node, which
/// then crops it. `unscaled` draws one texel per device pixel and ignores `fit`.
pub(crate) fn image_rect(node: [f32; 4], source: [u32; 2], fit: SurfaceFit, place: SurfacePlace) -> [f32; 4] {
    let (sw, sh) = (source[0] as f32, source[1] as f32);
    let scale = if place.unscaled {
        1.0
    } else {
        let (a, b) = (node[2] / sw, node[3] / sh);
        match fit {
            SurfaceFit::Fit => a.min(b),
            SurfaceFit::Fill => a.max(b),
        }
    };
    let [kx, ky] = place.share();
    let (w, h) = (sw * scale, sh * scale);
    let x = node[0] + (node[2] - w) * kx;
    let y = node[1] + (node[3] - h) * ky;
    // At unit scale favour sharp shared text over a subpixel translation of its image.
    if (scale - 1.0).abs() < 1e-6 { [x.round(), y.round(), w, h] } else { [x, y, w, h] }
}
