//! The displays and pointer a headless run pretends to have, so `place()` resolves there.
//! A `--script` object may set them next to its events:
//! `{"displays":[{"id":1,"frame":[0,0,1512,982],"scale":2,"primary":true}],"pointer":[800,400],"events":[]}`.
//! Frames and the pointer are points in one global, top-left space (macOS display bounds).

use serde_json::Value;

use crate::{DisplayTarget, NativeDisplayId, windows::PlaceRequest};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VirtualDisplay { pub id: u32, pub frame: [f64; 4], pub scale: f64, pub primary: bool }

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VirtualDesktop { pub displays: Vec<VirtualDisplay>, pub pointer: Option<(f64, f64)> }

impl VirtualDesktop {
    /// The single 1920x1080-pixel display headless window factories have always received.
    pub fn single(scale: f64) -> Self {
        let frame = [0.0, 0.0, 1920.0 / scale, 1080.0 / scale];
        Self { displays: vec![VirtualDisplay { id: 1, frame, scale, primary: true }], pointer: None }
    }

    /// The script's desktop; `single` where it sets no displays.
    #[cfg(test)]
    pub fn from_script(script: Option<&Value>, scale: f64) -> Result<Self, String> {
        Self::from_script_sized(script, scale, None)
    }

    /// `from_script`, with one display of `size` points where the script sets none.
    pub fn from_script_sized(script: Option<&Value>, scale: f64, size: Option<(f64, f64)>) -> Result<Self, String> {
        let mut desktop = Self::single(scale);
        if let Some((w, h)) = size { desktop.displays[0].frame = [0.0, 0.0, w, h]; }
        let Some(Value::Object(script)) = script else { return Ok(desktop) };
        if let Some(list) = script.get("displays") {
            let Value::Array(list) = list else { return Err("script: \"displays\" must be an array".into()) };
            desktop.displays = list.iter().enumerate().map(|(i, d)| {
                let frame = d["frame"].as_array().filter(|f| f.len() == 4)
                    .and_then(|f| Some([f[0].as_f64()?, f[1].as_f64()?, f[2].as_f64()?, f[3].as_f64()?]))
                    .filter(|f| f[2] > 0.0 && f[3] > 0.0)
                    .ok_or_else(|| format!("script: display {i} needs \"frame\": [x, y, width, height]"))?;
                let scale = match &d["scale"] {
                    Value::Null => scale,
                    s => s.as_f64().filter(|s| s.is_finite() && *s > 0.0)
                        .ok_or_else(|| format!("script: display {i} \"scale\" must be a number above 0"))?,
                };
                Ok(VirtualDisplay {
                    id: d["id"].as_u64().map_or(i as u32 + 1, |id| id as u32), frame, scale,
                    primary: d["primary"].as_bool().unwrap_or(false),
                })
            }).collect::<Result<_, String>>()?;
            if desktop.displays.is_empty() { return Err("script: \"displays\" is empty".into()); }
        }
        if let Some(p) = script.get("pointer") {
            desktop.pointer = Some(p.as_array().filter(|p| p.len() == 2).and_then(|p| Some((p[0].as_f64()?, p[1].as_f64()?)))
                .ok_or("script: \"pointer\" must be [x, y]")?);
        }
        Ok(desktop)
    }

    /// The primary display (the first when none is marked).
    pub fn primary(&self) -> &VirtualDisplay {
        self.displays.iter().find(|d| d.primary).unwrap_or(&self.displays[0])
    }

    /// A point in desktop pixels (`WindowOptions::position`) in global points, by the scale of
    /// the display it lies on, else the primary display's.
    pub fn to_points(&self, (x, y): (i32, i32)) -> (f32, f32) {
        let d = self.display_at(Some((x, y)));
        let (x, y) = (f64::from(x), f64::from(y));
        ((d.frame[0] + (x - d.frame[0] * d.scale) / d.scale) as f32, (d.frame[1] + (y - d.frame[1] * d.scale) / d.scale) as f32)
    }

    /// The display a point in desktop pixels lies on, else the primary display. Each display's
    /// pixels start at its point origin times its scale (as `displays` reports), so with mixed
    /// scales those boxes overlap: there the display whose point frame holds the point wins.
    pub fn display_at(&self, at: Option<(i32, i32)>) -> &VirtualDisplay {
        let Some((x, y)) = at else { return self.primary() };
        let (x, y) = (f64::from(x), f64::from(y));
        let inside = |[dx, dy, w, h]: [f64; 4]| x >= dx && y >= dy && x < dx + w && y < dy + h;
        let mut owners = self.displays.iter().filter(|d| inside(d.frame.map(|v| v * d.scale)));
        let first = owners.clone().next();
        owners.find(|d| inside(d.frame)).or(first).unwrap_or_else(|| self.primary())
    }

    /// The global frame a window of `size` points opens at: at `position` (desktop pixels),
    /// else centred on the primary display.
    pub fn open_at(&self, position: Option<(i32, i32)>, (w, h): (f32, f32)) -> [f32; 4] {
        match position {
            Some(p) => { let (x, y) = self.to_points(p); [x, y, w, h] }
            None => {
                let d = self.display_at(None).frame.map(|v| v as f32);
                [d[0] + ((d[2] - w) / 2.0).round(), d[1] + ((d[3] - h) / 2.0).round(), w, h]
            }
        }
    }

    /// What a window factory sees: desktop pixels, like native displays.
    pub fn displays(&self) -> Vec<crate::Display> {
        self.displays.iter().map(|d| crate::Display {
            id: None, name: Some("headless".into()),
            position: ((d.frame[0] * d.scale).round() as i32, (d.frame[1] * d.scale).round() as i32),
            size: ((d.frame[2] * d.scale).round() as u32, (d.frame[3] * d.scale).round() as u32),
            scale: d.scale, primary: d.primary,
        }).collect()
    }

    /// The display and global frame `request` gives a window of `size` points. Unknown
    /// displays fall back to primary, as natively.
    pub fn place(&self, request: &PlaceRequest, size: (f32, f32)) -> (u32, [f32; 4]) {
        let id = match &request.requested {
            DisplayTarget::Id(id) => id.native_id(),
            DisplayTarget::Native(id) => Some(*id),
            DisplayTarget::Primary | DisplayTarget::UnderPointer => None,
        }.map(|id| match id {
            NativeDisplayId::MacOS(n) | NativeDisplayId::X11Crtc(n) | NativeDisplayId::X11Output(n) | NativeDisplayId::Wayland(n) => i64::from(n),
            NativeDisplayId::Windows(n) => n as i64,
        });
        let bounds: Vec<_> = self.displays.iter().map(|d| [d.frame[0], d.frame[1], d.frame[0] + d.frame[2], d.frame[1] + d.frame[3]]).collect();
        let primary = self.displays.iter().position(|d| d.primary);
        let requested = id.and_then(|id| self.displays.iter().position(|d| i64::from(d.id) == id));
        let pointer = self.pointer.filter(|_| request.requested == DisplayTarget::UnderPointer);
        let d = &self.displays[crate::overlay::options::choose_display(&bounds, primary, requested, pointer).unwrap_or(0)];
        let local = crate::overlay::options::local_rect(PlaceRequest::at(size), Some(request.placement), (d.frame[2] as f32, d.frame[3] as f32));
        (d.id, [d.frame[0] as f32 + local[0], d.frame[1] as f32 + local[1], local[2], local[3]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::{Align, OverlayPlacement, ScreenEdge};

    #[test]
    fn headless_place_resolves_against_the_scripted_desktop() {
        let script = json!({
            "displays": [{"id": 1, "frame": [0, 0, 1512, 982], "scale": 2, "primary": true},
                         {"id": 7, "frame": [1512, -100, 1920, 1080], "scale": 1}],
            "pointer": [2000, 300], "events": [],
        });
        let desktop = VirtualDesktop::from_script(Some(&script), 2.0).unwrap();
        let top = OverlayPlacement::Edge { edge: ScreenEdge::Top, margin: 160.0, align: Align::Center, inset: 0.0 };
        let request = |display| PlaceRequest { requested: display, ..PlaceRequest::new(DisplayTarget::Primary, top) };
        assert_eq!(desktop.place(&request(DisplayTarget::Primary), (600.0, 80.0)), (1, [456.0, 160.0, 600.0, 80.0]));
        assert_eq!(desktop.place(&request(DisplayTarget::UnderPointer), (600.0, 80.0)), (7, [2172.0, 60.0, 600.0, 80.0]));
        assert_eq!(desktop.place(&request(DisplayTarget::Native(NativeDisplayId::MacOS(7))), (600.0, 80.0)).0, 7);
        assert_eq!(desktop.place(&request(DisplayTarget::Native(NativeDisplayId::MacOS(9))), (600.0, 80.0)).0, 1, "unknown ids fall back to primary");
        let full = PlaceRequest::new(DisplayTarget::Native(NativeDisplayId::MacOS(7)), OverlayPlacement::FullScreen);
        assert_eq!(desktop.place(&full, (600.0, 80.0)), (7, [1512.0, -100.0, 1920.0, 1080.0]));
        assert_eq!(desktop.displays()[1].position, (1512, -100));
        assert_eq!(desktop.displays()[0].size, (3024, 1964));

        let default = VirtualDesktop::from_script(None, 2.0).unwrap();
        assert_eq!(default.displays(), vec![crate::Display { id: None, name: Some("headless".into()),
            position: (0, 0), size: (1920, 1080), scale: 2.0, primary: true }], "unchanged factory display");
        assert!(VirtualDesktop::from_script(Some(&json!({"displays": [{"frame": [0, 0]}]})), 2.0).is_err());
        assert!(VirtualDesktop::from_script(Some(&json!({"displays": [{"frame": [0, 0, 10, 10], "scale": 0}]})), 2.0).is_err());
    }

    #[test]
    fn positions_on_mixed_scale_displays_convert_by_the_display_they_land_on() {
        // A Retina laptop with a 1x monitor to its right: their pixel boxes overlap from x = 1512.
        let script = json!({"displays": [{"id": 1, "frame": [0, 0, 1512, 982], "scale": 2, "primary": true},
                                         {"id": 2, "frame": [1512, 0, 1920, 1080], "scale": 1}]});
        let desktop = VirtualDesktop::from_script(Some(&script), 2.0).unwrap();
        let on = |p: (i32, i32)| (desktop.display_at(Some(p)).id, desktop.to_points(p));
        assert_eq!(on((200, 100)), (1, (100.0, 50.0)));
        assert_eq!(on((1300, 1800)), (1, (650.0, 900.0)));
        assert_eq!(on(desktop.displays()[1].position), (2, (1512.0, 0.0)), "the monitor's origin is on the monitor");
        assert_eq!(on((1612, 100)), (2, (1612.0, 100.0)));
        assert_eq!(on((-50, 0)).0, 1, "off every display: the primary");
        // Where a window opens, which its `El::on_screen_layout` rects start from.
        assert_eq!(desktop.open_at(None, (400.0, 300.0)), [556.0, 341.0, 400.0, 300.0], "centred on the primary");
        assert_eq!(desktop.open_at(Some((1612, 100)), (400.0, 300.0)), [1612.0, 100.0, 400.0, 300.0]);

        // The 2x display second and to the right: its pixels don't overlap the primary's.
        let script = json!({"displays": [{"id": 1, "frame": [0, 0, 1920, 1080], "scale": 1, "primary": true},
                                         {"id": 2, "frame": [1920, 0, 1512, 982], "scale": 2}]});
        let desktop = VirtualDesktop::from_script(Some(&script), 2.0).unwrap();
        let on = |p: (i32, i32)| (desktop.display_at(Some(p)).id, desktop.to_points(p));
        assert_eq!(on((3840 + 200, 1500)), (2, (2020.0, 750.0)));
        assert_eq!(on((100, 100)), (1, (100.0, 100.0)));
    }
}
