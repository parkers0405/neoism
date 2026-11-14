//! A variable font's hidden axes at a weight, as its named instances set them.

use swash::FontRef;

/// The coordinates a font's named instances give its hidden axes at `weight` (SF Mono's and SF
/// Italic's `YAXS`), interpolated along `wght` between the nearest instances and held at the
/// ends; only axes off their default. Core Text draws a weight at its instance (AppKit's regular
/// SF Mono is wght 400, YAXS 324.3); Parley sets only `wght`, which leaves a hidden axis at its
/// default, in SF Mono the Light instance's thin horizontal strokes. Instances at another value
/// of a visible axis (a width or optical size) only count when no instance is at the defaults;
/// of several at one weight (SF's grades), the one nearest the hidden defaults counts.
pub(super) fn hidden_axes(font: FontRef, weight: f32) -> Vec<parley::FontVariation> {
    let axes: Vec<_> = font.variations().map(|a| (a.tag(), a.default_value(), a.is_hidden())).collect();
    let Some(wght) = axes.iter().position(|a| a.0 == swash::tag_from_bytes(b"wght")) else { return Vec::new() };
    let hidden: Vec<usize> = (0..axes.len()).filter(|&i| i != wght && axes[i].2).collect();
    if hidden.is_empty() {
        return Vec::new();
    }
    let instances: Vec<Vec<f32>> = font.instances().map(|i| i.values().collect::<Vec<f32>>()).filter(|v| v.len() == axes.len()).collect();
    let at_defaults = |v: &&Vec<f32>| (0..axes.len()).all(|i| i == wght || axes[i].2 || (v[i] - axes[i].1).abs() < 1e-3);
    let mut points: Vec<(f32, Vec<f32>)> = instances.iter().filter(at_defaults).map(|v| (v[wght], hidden.iter().map(|&i| v[i]).collect())).collect();
    if points.is_empty() {
        points = instances.iter().map(|v| (v[wght], hidden.iter().map(|&i| v[i]).collect())).collect();
    }
    // Of instances at one weight (SF's grades), the one nearest the hidden defaults.
    let off_default = |v: &[f32]| hidden.iter().zip(v).map(|(&i, x)| (x - axes[i].1).abs()).sum::<f32>();
    points.sort_by(|a, b| a.0.total_cmp(&b.0).then(off_default(&a.1).total_cmp(&off_default(&b.1))));
    points.dedup_by(|b, a| a.0 == b.0);
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return Vec::new() };
    let values: Vec<f32> = if weight <= first.0 {
        first.1.clone()
    } else if weight >= last.0 {
        last.1.clone()
    } else {
        let k = points.iter().position(|p| p.0 >= weight).unwrap_or(points.len() - 1);
        let (lo, hi) = (&points[k - 1], &points[k]);
        let t = (weight - lo.0) / (hi.0 - lo.0);
        lo.1.iter().zip(&hi.1).map(|(a, b)| a + (b - a) * t).collect()
    };
    hidden
        .iter()
        .zip(values)
        .filter(|&(&i, v)| (v - axes[i].1).abs() >= 1e-3)
        .map(|(&i, v)| parley::FontVariation::new(parley::setting::Tag::new(&axes[i].0.to_be_bytes()), v))
        .collect()
}
