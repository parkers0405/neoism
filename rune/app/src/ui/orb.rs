//! Presence orbs (`shared/src/editor/crdt/presence_avatar.rs`): a person's profile picture is a
//! round pixel-plasma grid seeded from their display name, so the same person wears the same orb
//! everywhere. Cell geometry is fixed per orb; only colours flow. An orb is one picture
//! (`rune::Pixels`) that `tick` repaints in place, so no view re-runs and it is a single
//! element: a hundred-odd live-coloured cells per orb made user rows most of a transcript's
//! elements, built and diffed again on every commit.

use rune::{El, Pixels, el};
use std::cell::RefCell;
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::rc::Rc;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Profile {
    hues: [f32; 5],
    f: [f32; 4],
    s: [f32; 4],
    p: [f32; 2],
    grid: u32,
}

fn fmod(a: f32, n: f32) -> f32 { ((a % n) + n) % n }

fn lerp_hue(a: f32, b: f32, t: f32) -> f32 { fmod(a + (fmod(b - a + 180.0, 360.0) - 180.0) * t, 360.0) }

/// FNV-1a over UTF-16 units, as the original (and Synapse) hash names.
fn hash_seed(seed: &str) -> u32 {
    seed.encode_utf16().fold(2_166_136_261u32, |h, unit| (h ^ u32::from(unit)).wrapping_mul(16_777_619))
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        s as f32 / 4_294_967_296.0
    }
}

impl Profile {
    fn from_seed(seed: &str) -> Profile {
        let seed = if seed.is_empty() { " " } else { seed };
        let h = hash_seed(seed);
        let mut r = Rng(if h == 0 { 1 } else { h });
        let base = (h % 360) as f32;
        let hues = [
            base,
            fmod(base + 26.0 + r.next() * 24.0, 360.0),
            fmod(base + 58.0 + r.next() * 30.0, 360.0),
            fmod(base - 40.0 - r.next() * 24.0, 360.0),
            fmod(base + 165.0 + r.next() * 40.0, 360.0),
        ];
        let f = [5.0 + r.next() * 7.0, 5.0 + r.next() * 7.0, 4.0 + r.next() * 6.0, 6.0 + r.next() * 10.0];
        let s = [0.5 + r.next() * 0.9, 0.5 + r.next() * 0.9, 0.4 + r.next() * 0.8, 0.6 + r.next() * 1.1];
        let p = [r.next() * TAU, r.next() * TAU];
        let grid = 11 + (r.next() * 4.0).floor() as u32;
        Profile { hues, f, s, p, grid }
    }

    /// The cells inside the unit circle: grid position and normalised centre.
    fn cells(&self) -> Vec<(u32, u32, f32, f32)> {
        let g = self.grid as f32;
        (0..self.grid).flat_map(|j| (0..self.grid).map(move |i| (i, j))).filter_map(|(i, j)| {
            let nx = ((i as f32 + 0.5) / g) * 2.0 - 1.0;
            let ny = ((j as f32 + 0.5) / g) * 2.0 - 1.0;
            (nx.hypot(ny) <= 1.02).then_some((i, j, nx, ny))
        }).collect()
    }

    /// The cell's sRGB colour at time `t`.
    fn color(&self, nx: f32, ny: f32, t: f32) -> [f32; 3] {
        let dist = nx.hypot(ny);
        let mut p = (nx * self.f[0] + t * self.s[0] + self.p[0]).sin()
            + (ny * self.f[1] - t * self.s[1] + self.p[1]).sin()
            + ((nx + ny) * self.f[2] + t * self.s[2]).sin()
            + (dist * self.f[3] - t * self.s[3]).sin();
        p = (p + 4.0) / 8.0;
        let idx = p * self.hues.len() as f32;
        let lo = idx.floor() as usize;
        let hue = lerp_hue(self.hues[lo % 5], self.hues[(lo + 1) % 5], idx - lo as f32);
        let rim = 1.0 - ((dist - 0.62) / 0.38).max(0.0) * 0.55;
        hsl(hue, 0.88, (34.0 + p * 40.0) * rim / 100.0)
    }
}

fn hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let l = l.clamp(0.0, 1.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = fmod(h, 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 { 0 => (c, x, 0.0), 1 => (x, c, 0.0), 2 => (0.0, c, x), 3 => (0.0, x, c), 4 => (x, 0.0, c), _ => (c, 0.0, x) };
    let m = l - c / 2.0;
    [r + m, g + m, b + m]
}

/// Texels per point. Cell edges snap to half points, so they fall on whole texels, and on
/// whole device pixels at a window scale of 2.
const TEXELS: f32 = 2.0;

/// One person's orb at one size: its cells as texel rects with their normalised centres, and
/// the picture they are painted into.
struct Orb {
    profile: Profile,
    side: u32,
    cells: Vec<([usize; 4], f32, f32)>,
    pixels: Pixels,
}

impl Orb {
    fn new(profile: Profile, side: u32, t: f32) -> Orb {
        let edge = |k: u32| ((k as f32 * side as f32 / profile.grid as f32).round() as usize).min(side as usize);
        let cells = profile.cells().into_iter().map(|(i, j, nx, ny)| ([edge(i), edge(j), edge(i + 1), edge(j + 1)], nx, ny)).collect();
        let orb = Orb { profile, side, cells, pixels: Pixels::new() };
        orb.paint(t);
        orb
    }

    /// Paint every cell's colour at time `t`.
    fn paint(&self, t: f32) { self.pixels.set(self.side, self.side, self.picture(t)); }

    /// The orb at time `t` as RGBA texels; outside the disc stays clear.
    fn picture(&self, t: f32) -> Vec<u8> {
        let side = self.side as usize;
        let mut rgba = vec![0u8; side * side * 4];
        for ([x0, y0, x1, y1], nx, ny) in &self.cells {
            let [r, g, b] = self.profile.color(*nx, *ny, t).map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
            for y in *y0..*y1 {
                for texel in rgba[(y * side + x0) * 4..(y * side + x1) * 4].chunks_exact_mut(4) { texel.copy_from_slice(&[r, g, b, 255]); }
            }
        }
        rgba
    }
}

/// Every orb shown so far, by seed and size, and the clock they flow on.
pub struct Orbs {
    orbs: RefCell<HashMap<(String, u32), Rc<Orb>>>,
    epoch: Instant,
    /// When the colours last flowed: the plasma drifts at most ~2 rad/s, so it steps at
    /// `STEP` rather than repainting every vblank.
    last: std::cell::Cell<Option<Instant>>,
}

/// The orbs' frame interval (30 Hz).
pub const STEP: std::time::Duration = std::time::Duration::from_millis(33);

impl Default for Orbs {
    fn default() -> Orbs { Orbs { orbs: Default::default(), epoch: Instant::now(), last: Default::default() } }
}

impl Orbs {
    fn seconds(&self) -> f32 { self.epoch.elapsed().as_secs_f32() }

    fn orb(&self, seed: &str, side: u32) -> Rc<Orb> {
        let key = (seed.to_owned(), side);
        if let Some(orb) = self.orbs.borrow().get(&key) { return orb.clone(); }
        let orb = Rc::new(Orb::new(Profile::from_seed(seed), side, self.seconds()));
        self.orbs.borrow_mut().insert(key, orb.clone());
        orb
    }

    /// The orb for `seed`, `size` points square.
    pub fn view(&self, seed: &str, size: f32) -> El {
        let orb = self.orb(seed, (size * TEXELS).round().max(1.0) as u32);
        el().w(size).h(size).pixels(&orb.pixels)
    }

    /// Flow every orb to now, at most every `STEP`. Returns when to tick again, `None` with
    /// no orb.
    pub fn tick(&self) -> Option<std::time::Duration> {
        if self.orbs.borrow().is_empty() { return None; }
        let now = Instant::now();
        if let Some(due) = self.last.get().map(|at| at + STEP).filter(|due| *due > now) { return Some(due - now); }
        self.last.set(Some(now));
        let t = self.seconds();
        for orb in self.orbs.borrow().values() { orb.paint(t); }
        Some(STEP)
    }
}

/// The sender identity on a user message (`user_message_orb_identity`): the local person is
/// "You"; a message with no author is never assumed to be ours.
pub fn identity(author: Option<&str>, local: &str) -> (String, String) {
    match author.map(str::trim).filter(|a| !a.is_empty()) {
        Some(name) if name == local.trim() => (name.to_owned(), "You".to_owned()),
        Some(name) => (name.to_owned(), name.to_owned()),
        None => ("unknown-user".to_owned(), "Unknown user".to_owned()),
    }
}

/// `NEOISM_DISPLAY_NAME`, then `presence.display-name`, then the host name (`presence.rs`).
pub fn local_display_name(config_name: Option<&str>) -> String {
    let env = std::env::var("NEOISM_DISPLAY_NAME").ok();
    let host = crate::startup::hostname();
    [env.as_deref(), config_name, Some(host.trim())].into_iter().flatten().map(str::trim)
        .find(|name| !name.is_empty()).map(|name| name.chars().take(32).collect()).unwrap_or_else(|| "neoism".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_orb_and_round_silhouette() {
        let a = Profile::from_seed("piss-desktop");
        assert_eq!(a, Profile::from_seed("piss-desktop"));
        assert_ne!(a, Profile::from_seed("other-host"));
        assert!((11..=14).contains(&a.grid));
        let cells = a.cells();
        assert!(cells.len() < (a.grid * a.grid) as usize, "corners are cut");
        assert!(cells.iter().all(|c| c.2.hypot(c.3) <= 1.02));
    }

    #[test]
    fn an_orb_is_one_picture_with_whole_texel_cells() {
        let orb = Orb::new(Profile::from_seed("piss-desktop"), 56, 1.5);
        let picture = orb.picture(1.5);
        assert_eq!(picture.len(), 56 * 56 * 4);
        let alpha = |x: usize, y: usize| picture[(y * 56 + x) * 4 + 3];
        // The disc is opaque, the corners it cuts are clear.
        assert_eq!(alpha(28, 28), 255);
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(55, 55), 0);
        // Cells tile the grid edge to edge on whole texels.
        assert!(orb.cells.iter().all(|([x0, y0, x1, y1], _, _)| x0 < x1 && y0 < y1 && *x1 <= 56 && *y1 <= 56));
        // The colours flow: another moment paints another picture of the same shape.
        let later = orb.picture(2.5);
        assert_ne!(picture, later);
        assert!(picture.chunks_exact(4).zip(later.chunks_exact(4)).all(|(a, b)| a[3] == b[3]));
    }

    #[test]
    fn one_picture_per_person_and_size() {
        let orbs = Orbs::default();
        let a = orbs.orb("Parker", 56);
        assert!(Rc::ptr_eq(&a, &orbs.orb("Parker", 56)));
        assert!(!Rc::ptr_eq(&a, &orbs.orb("Parker", 40)));
        assert!(!Rc::ptr_eq(&a, &orbs.orb("Peer", 56)));
    }

    #[test]
    fn identity_labels() {
        assert_eq!(identity(Some("Parker"), "Parker").1, "You");
        assert_eq!(identity(Some("Peer"), "Parker").1, "Peer");
        assert_eq!(identity(None, "Parker").0, "unknown-user");
    }
}
