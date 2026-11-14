//! A stretch of a clip as a folder someone else can read: `trace.jsonl` (motion-lint's format),
//! `contact.png` (a contact sheet drawn from the recorded rects) and `summary.md`.

use std::fmt::Write as _;
use std::io::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::clip::{Change, Clip, Shot};

/// What [`Clip::write_bundle`] wrote.
#[derive(Clone, Debug)]
pub struct Bundle {
    dir: PathBuf,
    summary: String,
    frames: Range<usize>,
}

impl Bundle {
    /// The folder.
    pub fn dir(&self) -> &Path { &self.dir }
    /// `summary.md`'s text: the note, the stretch, the jumps and what moved most.
    pub fn summary(&self) -> &str { &self.summary }
    /// The frames it holds.
    pub fn frames(&self) -> Range<usize> { self.frames.clone() }
    /// The files in it.
    pub fn files(&self) -> [PathBuf; 3] {
        ["summary.md", "trace.jsonl", "contact.png"].map(|f| self.dir.join(f))
    }
}

const TILE_W: u32 = 320;
const TILES: usize = 12;
const COLS: usize = 4;

impl Clip {
    /// Writes frames `range` (clamped to the clip) into `dir`, created if missing, with `note`
    /// at the top of the summary.
    pub fn write_bundle(&self, dir: &Path, range: Range<usize>, note: &str) -> std::io::Result<Bundle> {
        let end = range.end.min(self.len());
        let range = range.start.min(end.saturating_sub(1))..end.max(1).min(self.len());
        std::fs::create_dir_all(dir)?;
        let changes: Vec<Vec<Change>> = range.clone().map(|k| self.changes(k)).collect();
        self.write_trace(&dir.join("trace.jsonl"), range.clone())?;
        let tiles = self.pick_tiles(range.clone());
        self.write_contact(&dir.join("contact.png"), &tiles)?;
        let summary = self.summary_text(range.clone(), note, &changes, &tiles);
        std::fs::write(dir.join("summary.md"), &summary)?;
        Ok(Bundle { dir: dir.to_owned(), summary, frames: range })
    }

    fn write_trace(&self, path: &Path, range: Range<usize>) -> std::io::Result<()> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        let hz = (1.0 / self.frame_interval()).round();
        writeln!(out, r#"{{"type":"header","launch":"rune-rewind","width":{},"height":{},"hz":{hz},"scale":1,"platform":"rune","units":"pt","clock":"wall"}}"#,
            self.viewport.0, self.viewport.1)?;
        let mut described = vec![false; self.names.len()];
        let start_t = self.time(range.start);
        let first = range.start;
        let mut last: Vec<Option<Shot>> = vec![None; self.names.len()];
        let mut state: Vec<Option<Shot>> = vec![None; self.names.len()];
        for (id, shot) in self.before(first) {
            state[id as usize] = Some(shot);
        }
        for k in range {
            for c in self.changes(k) { state[c.id as usize] = c.after; }
            let mut line = String::new();
            let _ = write!(line, r#"{{"t":{:.3},"n":1"#, (self.time(k) - start_t) * 1000.0);
            let (mut metas, mut elems, mut gone) = (Vec::new(), Vec::new(), Vec::new());
            for (i, now) in state.iter().enumerate() {
                if *now == last[i] { continue; }
                match now {
                    Some(s) => {
                        if !described[i] {
                            described[i] = true;
                            metas.push(format!(r#"{{"id":{},"cls":"rune","nm":{},"host":"rune"}}"#, i + 1, json_str(&self.names[i])));
                        }
                        let [x, y, w, h] = s.rect;
                        elems.push(format!("[{},{x},{y},{w},{h},{},0,0]", i + 1, s.opacity));
                    }
                    None => gone.push((i + 1).to_string()),
                }
                last[i] = *now;
            }
            if !metas.is_empty() { let _ = write!(line, r#","m":[{}]"#, metas.join(",")); }
            if !elems.is_empty() { let _ = write!(line, r#","e":[{}]"#, elems.join(",")); }
            if !gone.is_empty() { let _ = write!(line, r#","g":[{}]"#, gone.join(",")); }
            line.push('}');
            writeln!(out, "{line}")?;
        }
        out.flush()
    }

    /// Up to `TILES` frames: every jump first, then the busiest of the rest, in order.
    fn pick_tiles(&self, range: Range<usize>) -> Vec<usize> {
        let mut picked: Vec<usize> = range.clone().filter(|&k| self.has_jump(k)).take(TILES).collect();
        let mut rest: Vec<usize> = range.clone().filter(|&k| !self.has_jump(k) && self.activity(k) > 0.0).collect();
        rest.sort_by(|a, b| self.activity(*b).total_cmp(&self.activity(*a)));
        picked.extend(rest.into_iter().take(TILES.saturating_sub(picked.len())));
        if picked.is_empty() { picked.push(range.end.saturating_sub(1)); }
        picked.sort_unstable();
        picked
    }

    fn write_contact(&self, path: &Path, tiles: &[usize]) -> std::io::Result<()> {
        let (vw, vh) = (self.viewport.0.max(1.0), self.viewport.1.max(1.0));
        let scale = TILE_W as f32 / vw;
        let tile_h = ((vh * scale).round() as u32).max(1);
        let cols = COLS.min(tiles.len()).max(1);
        let rows = tiles.len().div_ceil(cols);
        let gap = 8;
        let (w, h) = (cols as u32 * (TILE_W + gap) + gap, rows as u32 * (tile_h + gap) + gap);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for p in px.chunks_exact_mut(4) { p.copy_from_slice(&[10, 10, 12, 255]); }
        let mut canvas = Canvas { px: &mut px, w, h };
        for (n, &k) in tiles.iter().enumerate() {
            let (ox, oy) = ((n % cols) as u32 * (TILE_W + gap) + gap, (n / cols) as u32 * (tile_h + gap) + gap);
            canvas.fill(ox, oy, TILE_W, tile_h, [24, 24, 28]);
            let at = |r: [f32; 4]| [ox as f32 + r[0] * scale, oy as f32 + r[1] * scale, r[2] * scale, r[3] * scale];
            let clip = [ox as f32, oy as f32, TILE_W as f32, tile_h as f32];
            for (_, s) in self.frame(k) {
                let grey = (40.0 + 80.0 * s.opacity.clamp(0.0, 1.0)) as u8;
                canvas.outline(at(s.rect), clip, [grey, grey, grey + 6]);
            }
            for c in self.changes(k).iter().rev() {
                let Some(s) = c.after.or(c.before) else { continue };
                let color = if c.jump { [255, 84, 84] } else { [255, 204, 64] };
                if let Some(b) = c.before.filter(|_| c.after.is_some()) { canvas.outline(at(b.rect), clip, [110, 90, 40]); }
                canvas.outline(at(s.rect), clip, color);
            }
        }
        crate::render::write_png(path, w, h, &px)
    }

    fn summary_text(&self, range: Range<usize>, note: &str, changes: &[Vec<Change>], tiles: &[usize]) -> String {
        let mut s = String::from("# Rewind clip\n\n");
        if !note.trim().is_empty() { let _ = writeln!(s, "> {}\n", note.trim().replace('\n', "\n> ")); }
        let hz = (1.0 / self.frame_interval()).round();
        let _ = writeln!(s, "Frames {}–{} of {} ({:.2} s to {:.2} s before the freeze, {} frames at {hz} Hz), window {}×{} pt.\n",
            range.start, range.end.saturating_sub(1), self.len(), self.time(range.start), self.time(range.end.saturating_sub(1)),
            range.len(), self.viewport.0, self.viewport.1);
        let jumps: Vec<String> = range.clone().zip(changes).flat_map(|(k, cs)| {
            cs.iter().filter(|c| c.jump).map(move |c| format!("- frame {k} ({:.2} s): {}", self.time(k), c.label()))
        }).collect();
        if !jumps.is_empty() {
            let _ = writeln!(s, "## Jumps\n\nMoved much further in one frame than in the frames either side.\n\n{}\n", jumps.join("\n"));
        }
        // Per element: how many frames it changed in, and its largest single-frame move.
        let mut most: Vec<(u32, usize, f32, usize, String)> = Vec::new();
        for (k, cs) in range.clone().zip(changes) {
            for c in cs {
                let d = c.distance();
                match most.iter_mut().find(|m| m.0 == c.id) {
                    Some(m) => { m.1 += 1; if d > m.2 { (m.2, m.3, m.4) = (d, k, c.label()); } }
                    None => most.push((c.id, 1, d, k, c.label())),
                }
            }
        }
        most.sort_by(|a, b| b.2.total_cmp(&a.2));
        if !most.is_empty() {
            let _ = writeln!(s, "## Moved most\n");
            for (_, n, _, k, label) in most.iter().take(12) {
                let _ = writeln!(s, "- {label} at frame {k} (changed in {n} frames)");
            }
            let _ = writeln!(s);
        }
        let _ = writeln!(s, "## Files\n\n- `trace.jsonl`: every element's rect and opacity per frame, motion-lint's format (`motion-lint analyze trace.jsonl`).\n- `contact.png`: frames {} drawn from the recorded rects, left to right. Grey: on screen. Yellow: changed in that frame, its old place dimmer. Red: jumped.",
            tiles.iter().map(usize::to_string).collect::<Vec<_>>().join(", "));
        s
    }
}

struct Canvas<'a> {
    px: &'a mut [u8],
    w: u32,
    h: u32,
}

impl Canvas<'_> {
    fn put(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 { return; }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        self.px[i..i + 3].copy_from_slice(&c);
    }

    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, c: [u8; 3]) {
        for yy in y..(y + h).min(self.h) { for xx in x..(x + w).min(self.w) { self.put(xx as i64, yy as i64, c); } }
    }

    /// A one-pixel outline of `r`, cut to `clip`.
    fn outline(&mut self, r: [f32; 4], clip: [f32; 4], c: [u8; 3]) {
        let (x0, y0) = (r[0].max(clip[0]), r[1].max(clip[1]));
        let (x1, y1) = ((r[0] + r[2]).min(clip[0] + clip[2] - 1.0), (r[1] + r[3]).min(clip[1] + clip[3] - 1.0));
        if x1 < x0 || y1 < y0 { return; }
        let (ax, ay, bx, by) = (x0.round() as i64, y0.round() as i64, x1.round() as i64, y1.round() as i64);
        for x in ax..=bx {
            if r[1] >= clip[1] { self.put(x, ay, c); }
            if r[1] + r[3] <= clip[1] + clip[3] { self.put(x, by, c); }
        }
        for y in ay..=by {
            if r[0] >= clip[0] { self.put(ax, y, c); }
            if r[0] + r[2] <= clip[0] + clip[2] { self.put(bx, y, c); }
        }
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => { let _ = write!(out, "\\u{:04x}", c as u32); }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
