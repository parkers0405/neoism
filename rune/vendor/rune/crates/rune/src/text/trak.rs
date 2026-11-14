//! The AAT `trak` table: tracking that depends on the point size. Core Text applies it at track 0
//! to fonts that also have a `STAT` table (the macOS system font among them), and so does
//! HarfRust, but Parley hands HarfRust the font size in device pixels as the point size, so on a
//! 2x display 14 pt text was tracked as 28 pt text. [`Trak::spacing`] corrects for that.

use std::collections::HashMap;
use std::rc::Rc;

use parley::fontique::Blob;
use swash::FontRef;

/// A font's horizontal track-0 tracking, in font units, at each point size its table lists.
pub(super) struct Trak {
    sizes: Vec<(f32, f32)>,
    units_per_em: f32,
}

impl Trak {
    /// The table of a font HarfRust tracks: one with both `trak` and `STAT`.
    pub(super) fn of(font: FontRef<'_>) -> Option<Trak> {
        font.table(swash::tag_from_bytes(b"STAT"))?;
        Trak::parse(font.table(swash::tag_from_bytes(b"trak"))?, font.metrics(&[]).units_per_em)
    }

    /// Track 0 of a `trak` table, or the interpolation between the tracks around it, read the way
    /// HarfRust reads it: a table it can't read tracks nothing, and a track whose values it can't
    /// read tracks by 0. Fonts can be untrusted, so every offset is checked.
    fn parse(data: &[u8], units_per_em: u16) -> Option<Trak> {
        if units_per_em == 0 {
            return None;
        }
        let bytes = |at: usize, len: usize| data.get(at..at.checked_add(len)?);
        let u16_at = |at: usize| Some(u16::from_be_bytes(bytes(at, 2)?.try_into().ok()?));
        let fixed = |b: &[u8]| i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32 / 65536.0;
        let horizontal = u16_at(6)? as usize;
        if horizontal == 0 {
            return None;
        }
        let (tracks, sizes) = (u16_at(horizontal)? as usize, u16_at(horizontal.checked_add(2)?)? as usize);
        let size_table = u32::from_be_bytes(bytes(horizontal.checked_add(4)?, 4)?.try_into().ok()?);
        let entries = bytes(horizontal.checked_add(8)?, tracks.checked_mul(8)?)?;
        let size_values = bytes(usize::try_from(size_table).ok()?, sizes.checked_mul(4)?)?;
        if tracks == 0 || sizes == 0 {
            return None;
        }
        let entry = |i: usize| &entries[i * 8..i * 8 + 8];
        let track = |i: usize| fixed(entry(i));
        let values = |i: usize| -> Vec<f32> {
            let at = u16::from_be_bytes([entry(i)[6], entry(i)[7]]) as usize;
            match bytes(at, sizes * 2) {
                Some(b) => b.chunks_exact(2).map(|v| i16::from_be_bytes([v[0], v[1]]) as f32).collect(),
                None => vec![0.0; sizes],
            }
        };
        let mut i = 0;
        while i + 1 < tracks && track(i + 1) <= 0.0 {
            i += 1;
        }
        let mut j = tracks - 1;
        while j > 0 && track(j - 1) >= 0.0 {
            j -= 1;
        }
        let (a, b) = (values(i), if i == j { Vec::new() } else { values(j) });
        let (t0, t1) = (track(i), track(j));
        let t = if i == j || (t1 - t0).abs() < f32::EPSILON { 0.0 } else { -t0 / (t1 - t0) };
        let sizes = size_values
            .chunks_exact(4)
            .zip(&a)
            .enumerate()
            .map(|(s, (size, &a))| (fixed(size), b.get(s).map_or(a, |&b| a + t * (b - a))))
            .collect();
        Some(Trak { sizes, units_per_em: units_per_em as f32 })
    }

    /// Tracking at `points`, in font units: interpolated between the listed sizes and held
    /// beyond them.
    fn units(&self, points: f32) -> f32 {
        let Some(i) = self.sizes.iter().position(|&(size, _)| size >= points) else {
            return self.sizes.last().map_or(0.0, |s| s.1);
        };
        if i == 0 {
            return self.sizes[0].1;
        }
        let ((s0, v0), (s1, v1)) = (self.sizes[i - 1], self.sizes[i]);
        if (s1 - s0).abs() < f32::EPSILON {
            return (v0 + v1) / 2.0;
        }
        v0 + (points - s0) / (s1 - s0) * (v1 - v0)
    }

    /// Letter spacing, in device px, that turns the tracking HarfRust applied to a run at `px`
    /// device pixels into Core Text's tracking for `points`-point text (its point size, or the
    /// size an explicit optical size stands for) scaled to the run, or removes it when `on` is
    /// false. HarfRust rounds tracking to whole font units; Core Text truncates.
    pub(super) fn spacing(&self, px: f32, points: f32, on: bool) -> f32 {
        let applied = self.units(px).round();
        let wanted = if on { self.units(points).trunc() } else { 0.0 };
        (wanted - applied) * px / self.units_per_em
    }
}

#[cfg(test)]
mod tests {
    use super::Trak;

    /// Part of the macOS system font's track-0 row.
    fn system() -> Trak {
        let sizes = [(12.0, 0.0), (13.0, -12.0), (14.0, -22.0), (24.0, 6.0), (28.0, 28.0), (32.0, 26.0), (36.0, 21.0)];
        Trak { sizes: sizes.to_vec(), units_per_em: 2048.0 }
    }

    #[test]
    fn spacing_moves_tracking_from_the_pixel_size_to_the_point_size() {
        let t = system();
        // 14 pt at 2x: HarfRust tracked 28 px by +28 units; 14 pt wants -22.
        assert!((t.spacing(28.0, 14.0, true) - (-22.0 - 28.0) * 28.0 / 2048.0).abs() < 1e-5);
        // At 1x, listed sizes need nothing.
        assert_eq!(t.spacing(14.0, 14.0, true), 0.0);
        // Off removes what HarfRust applied.
        assert!((t.spacing(14.0, 14.0, false) - 22.0 * 14.0 / 2048.0).abs() < 1e-5);
        // Between listed sizes Core Text truncates and HarfRust rounds: 33 pt is 24.75 units.
        assert!((t.spacing(33.0, 33.0, true) - (24.0 - 25.0) * 33.0 / 2048.0).abs() < 1e-5);
        // An explicit optical size of 14 pt at 28 pt, 2x: 14 pt's -22 units, at the real size.
        assert!((t.spacing(56.0, 14.0, true) - (-22.0 - t.units(56.0).round()) * 56.0 / 2048.0).abs() < 1e-5);
        // Beyond the table the ends hold.
        assert_eq!(t.units(8.0), 0.0);
        assert_eq!(t.units(500.0), 21.0);
    }

    /// A `trak` table with one horizontal track per `(track, values)` at `sizes`.
    fn table(tracks: &[(f32, &[i16])], sizes: &[f32]) -> Vec<u8> {
        let fixed = |v: f32| ((v * 65536.0) as i32).to_be_bytes();
        let size_table = 12 + 8 + tracks.len() * 8;
        let values = size_table + sizes.len() * 4;
        let mut t = vec![0, 1, 0, 0, 0, 0, 0, 12, 0, 0, 0, 0];
        t.extend((tracks.len() as u16).to_be_bytes());
        t.extend((sizes.len() as u16).to_be_bytes());
        t.extend((size_table as u32).to_be_bytes());
        for (i, (track, _)) in tracks.iter().enumerate() {
            t.extend(fixed(*track));
            t.extend([0, 0]);
            t.extend(((values + i * sizes.len() * 2) as u16).to_be_bytes());
        }
        t.extend(sizes.iter().flat_map(|&s| fixed(s)));
        t.extend(tracks.iter().flat_map(|(_, v)| v.iter().flat_map(|v| v.to_be_bytes())));
        t
    }

    fn put(t: &mut [u8], at: usize, bytes: &[u8]) {
        t[at..at + bytes.len()].copy_from_slice(bytes);
    }

    /// Tracking that can't go wrong: finite, and 0 when there's no table.
    fn sane(t: Option<Trak>) -> f32 {
        let spacing = t.map_or(0.0, |t| t.spacing(28.0, 14.0, true) + t.spacing(9.0, 9.0, false));
        assert!(spacing.is_finite(), "{spacing}");
        spacing
    }

    #[test]
    fn parse_reads_track_zero_or_interpolates_around_it() {
        let sizes = [12.0, 24.0];
        let t = Trak::parse(&table(&[(-1.0, &[-40, -20]), (0.0, &[0, 10]), (1.0, &[40, 20])], &sizes), 2048).unwrap();
        assert_eq!(t.sizes, [(12.0, 0.0), (24.0, 10.0)]);
        let t = Trak::parse(&table(&[(-1.0, &[-40, -20]), (1.0, &[40, 60])], &sizes), 2048).unwrap();
        assert_eq!(t.sizes, [(12.0, 0.0), (24.0, 20.0)]);
        let t = Trak::parse(&table(&[(0.5, &[8, 16])], &sizes), 2048).unwrap();
        assert_eq!(t.sizes, [(12.0, 8.0), (24.0, 16.0)]);
    }

    #[test]
    fn malformed_tables_track_nothing_without_panicking() {
        let good = table(&[(-1.0, &[-40, -20]), (0.0, &[0, 10]), (1.0, &[40, 20])], &[12.0, 24.0]);
        assert!(Trak::parse(&good, 0).is_none(), "zero units per em");
        for len in 0..good.len() {
            sane(Trak::parse(&good[..len], 2048));
        }
        let mut huge_offsets = good.clone();
        put(&mut huge_offsets, 6, &[0xff, 0xff]);
        assert!(Trak::parse(&huge_offsets, 2048).is_none());
        let mut huge_size_table = good.clone();
        put(&mut huge_size_table, 16, &[0xff; 4]);
        assert!(Trak::parse(&huge_size_table, 2048).is_none());
        let mut huge_counts = good.clone();
        put(&mut huge_counts, 12, &[0xff; 4]);
        assert!(Trak::parse(&huge_counts, 2048).is_none());
        // Track 0's values out of bounds: it tracks by 0, as in HarfRust.
        let mut bad_values = good.clone();
        put(&mut bad_values, 20 + 8 + 6, &[0xff, 0xff]);
        assert_eq!(Trak::parse(&bad_values, 2048).unwrap().sizes, [(12.0, 0.0), (24.0, 0.0)]);
        // A bad offset on a track that isn't used doesn't spoil the table.
        let mut bad_unused = good.clone();
        put(&mut bad_unused, 20 + 6, &[0xff, 0xff]);
        assert_eq!(Trak::parse(&bad_unused, 2048).unwrap().sizes, [(12.0, 0.0), (24.0, 10.0)]);
        for at in 0..good.len() {
            let mut t = good.clone();
            t[at] = 0xff;
            sane(Trak::parse(&t, 2048));
        }
    }
}

/// Fonts' `trak` tables (None for an untracked font), by blob and index.
pub(super) type Traks = HashMap<(u64, u32), Option<Rc<Trak>>>;

pub(super) fn font_trak(traks: &mut Traks, data: &Blob<u8>, index: u32) -> Option<Rc<Trak>> {
    traks
        .entry((data.id(), index))
        .or_insert_with(|| FontRef::from_index(data.as_ref(), index as usize).and_then(Trak::of).map(Rc::new))
        .clone()
}
