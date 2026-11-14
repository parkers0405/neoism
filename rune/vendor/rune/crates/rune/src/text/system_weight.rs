//! Which face of a system font family draws text of a given weight, as Chromium picks it on
//! Windows. Chromium finds a system family's face through Skia's DirectWrite font manager
//! (`IDWriteFontFamily::GetFirstMatchingFont`), and fallback text the same way
//! (`font_cache_skia_win.cc`): DirectWrite takes the face whose weight is nearest, and of two
//! as near, the one further from 400 on the side of the weight asked for. CSS's matching, which
//! Parley's font query runs, tries heavier faces first above 500, so Han text at 530 drew in
//! Microsoft YaHei Bold where Chromium draws Microsoft YaHei, and kana in Yu Gothic Bold where
//! Chromium draws Yu Gothic Medium. After shaping, [`corrections`] names the runs whose face
//! DirectWrite wouldn't pick, with the weight of the face it would, and the text shapes again
//! with those weights, which CSS then matches exactly.

use std::rc::Rc;

use parley::fontique::{Blob, FamilyId, FontStyle, FontWidth};
use parley::{FontContext, Layout};

use super::Ink;
use super::system_fonts::LoadedFiles;
use crate::runtime::HashMap as FxHashMap;

/// A face of a system family.
#[derive(Clone, Copy)]
pub(super) struct Sibling {
    weight: f32,
    style: FontStyle,
    width: FontWidth,
}

/// Each system face's family, by blob and face index, for the system families loaded so far.
#[derive(Default)]
pub(super) struct Faces {
    /// The faces of each face's family, and the face itself among them; None for faces of no
    /// system family (the app's own fonts).
    map: FxHashMap<(u64, u32), Option<(Rc<[Sibling]>, Sibling)>>,
    /// How many files `map` knew of; a file loaded since can add faces to a family.
    files: usize,
}

/// The runs of `layout` in a system face DirectWrite wouldn't pick for the weight they ask for
/// (`wanted` at a byte), each with the weight of the face it would.
pub(super) fn corrections(
    faces: &mut Faces,
    fcx: &mut FontContext,
    files: &LoadedFiles,
    layout: &Layout<Ink>,
    wanted: impl Fn(usize) -> f32,
) -> Vec<(std::ops::Range<usize>, f32)> {
    if faces.files != files.len() {
        faces.map.clear();
        faces.files = files.len();
    }
    let mut out: Vec<(std::ops::Range<usize>, f32)> = Vec::new();
    for line in layout.lines() {
        for run in line.runs() {
            let range = run.text_range();
            if range.is_empty() {
                continue;
            }
            let font = run.font();
            let Some((siblings, face)) = family_of(faces, fcx, files, &font.data, font.index) else { continue };
            let wanted = wanted(range.start);
            let near = nearest(wanted, siblings.iter().filter(|s| s.style == face.style && s.width == face.width).map(|s| s.weight));
            let Some(weight) = near.filter(|&w| w != face.weight) else { continue };
            match out.last_mut() {
                Some((last, w)) if last.end == range.start && *w == weight => last.end = range.end,
                _ => out.push((range, weight)),
            }
        }
    }
    out
}

/// DirectWrite's pick among `weights` for `wanted`: the nearest, and of two as near, the one
/// whose distance from 400 points the same way as `wanted`'s (`GetFirstMatchingFont`).
fn nearest(wanted: f32, weights: impl Iterator<Item = f32>) -> Option<f32> {
    let lean = |w: f32| (w - 400.0) * (wanted - 400.0);
    weights.min_by(|a, b| (a - wanted).abs().total_cmp(&(b - wanted).abs()).then_with(|| lean(*b).total_cmp(&lean(*a))))
}

/// The family of the face `index` of `data` among the loaded system families, with that face.
/// Families with a `wght` axis are left out: the weight asked for sets the axis.
fn family_of(faces: &mut Faces, fcx: &mut FontContext, files: &LoadedFiles, data: &Blob<u8>, index: u32) -> Option<(Rc<[Sibling]>, Sibling)> {
    let key = (data.id(), index);
    if let Some(found) = faces.map.get(&key) {
        return found.clone();
    }
    let mut ids: Vec<FamilyId> = files.values().flatten().copied().collect();
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        let Some(family) = fcx.collection.family(id) else { continue };
        if family.fonts().iter().any(|f| f.has_weight_axis()) {
            continue;
        }
        let siblings: Rc<[Sibling]> = family.fonts().iter().map(|f| Sibling { weight: f.weight().value(), style: f.style(), width: f.width() }).collect();
        let keys: Vec<Option<(u64, u32)>> = family.fonts().iter().map(|f| f.load(Some(&mut fcx.source_cache)).map(|b| (b.id(), f.index()))).collect();
        if !keys.contains(&Some(key)) {
            continue;
        }
        for (k, s) in keys.into_iter().zip(siblings.iter()) {
            if let Some(k) = k {
                faces.map.insert(k, Some((siblings.clone(), *s)));
            }
        }
        return faces.map.get(&key).cloned().flatten();
    }
    faces.map.insert(key, None);
    None
}
