//! `--crop KEY` and `--playground`'s stage: a `--snap` cropped to one element's rect.

use super::*;

/// The pixel rect `[x, y, w, h]` `--snap` keeps, or None for the whole frame: `--crop KEY`'s
/// element, else the playground's stage when `--playground` opened it without
/// `--whole-window`. On the stage, `KEY` is looked up there only. Rounded out to whole pixels
/// and clipped to the `w` by `h` frame.
pub(crate) fn snap_rect<S: 'static>(rt: &Runtime<S>, args: &Args, scale: f32, (w, h): (u32, u32)) -> Option<[u32; 4]> {
    let all = rt.elements();
    let stage = if args.playground.is_some() && !args.whole_window {
        all.iter().position(|e| !e.exiting && e.key.as_deref().is_some_and(crate::playground::is_stage))
    } else {
        None
    };
    let pixels = |i: usize| {
        let [x, y, rw, rh] = all[i].rect;
        let px = |v: f32, max: u32, round: fn(f32) -> f32| (round(v * scale).max(0.0) as u32).min(max);
        let (x0, y0, x1, y1) = (px(x, w, f32::floor), px(y, h, f32::floor), px(x + rw, w, f32::ceil), px(y + rh, h, f32::ceil));
        (x1 > x0 && y1 > y0).then(|| [x0, y0, x1 - x0, y1 - y0])
    };
    let Some(key) = &args.crop_key else {
        // A window too narrow for a stage snaps whole.
        return stage.and_then(pixels);
    };
    let found = match stage {
        Some(_) => crate::playground::stage_element(&all, key).unwrap_or_else(|| {
            die(&format!("--crop {key}: no element on the playground's stage has that key or path; --whole-window searches the whole window"))
        }),
        None => {
            let named = |i: &usize| !all[*i].exiting && (all[*i].key.as_deref() == Some(key.as_str()) || all[*i].path == *key);
            (0..all.len()).find(named).unwrap_or_else(|| die(&format!("--crop {key}: no element has that key or path")))
        }
    };
    Some(pixels(found).unwrap_or_else(|| die(&format!("--crop {key}: the element is empty or outside the frame"))))
}

/// The `rect` of an RGBA frame `w` pixels wide.
pub(crate) fn crop(px: &[u8], w: u32, [x, y, cw, ch]: [u32; 4]) -> Vec<u8> {
    let row = |r: u32| {
        let start = ((y + r) * w + x) as usize * 4;
        &px[start..start + cw as usize * 4]
    };
    (0..ch).flat_map(row).copied().collect()
}
