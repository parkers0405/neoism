//! outline.wgsl's coverage on the CPU, line for line in f32: masks placed before any renderer
//! can fill them (the first frame, a runtime drawn without one) come out as the GPU fills them.

use super::store::OutlineStore;

/// `outline_coverage`: how much of a pixel `2 * hw` rest px square at `local` (rest px from the
/// pen, y up) the glyph of record `rec` covers, `scale` rest px a font unit, dilated by `r` rest
/// px and hinted by record `hint` (0: none).
pub(crate) fn coverage(store: &OutlineStore, rec: u32, hint: u32, local: [f32; 2], scale: f32, r: f32, hw: f32) -> f32 {
    let t = |i: u32| store.texel(i);
    let f = f32::from_bits;
    let head = t(rec);
    let (count, y0, inv_h) = (head[1], f(head[2]), f(head[3]));
    let h = (hint != 0).then(|| [t(hint), t(hint + 1), t(hint + 2)].map(|v| v.map(f)));
    let hy = |y: f32| h.map_or(y, |[a, b, c]| hint_y(y, a, b, c));
    let reach = if h.is_some() { super::HINT_REACH } else { 0.0 };
    let (b0, b1) = if count > 1 {
        let last = (count - 1) as f32;
        let band = |u: f32| ((u / scale - y0) * inv_h).floor().clamp(0.0, last) as u32;
        (band(local[1] - hw - 2.0 * r - reach), band(local[1] + hw + reach))
    } else {
        (0, 0)
    };
    let mut acc = 0.0f32;
    for band in b0..=b1 {
        let (e0, e1) = if count > 1 { ((y0 + band as f32 / inv_h) * scale, (y0 + (band + 1) as f32 / inv_h) * scale) } else { (-3.0e38, 3.0e38) };
        let entry = t(head[0] + band);
        let sorted = entry[1] > super::build::SORT_MIN as u32;
        for k in 0..entry[1] {
            let (t0, t1) = (t(entry[0] + 2 * k), t(entry[0] + 2 * k + 1));
            if f(t1[3]) * scale + r - local[0] <= -hw {
                if sorted { break; }
                continue;
            }
            let code = t1[2];
            let (oa, ob) = (moved(code & 15, r), moved((code >> 4) & 15, r));
            let at = |x: u32, y: u32, o: [f32; 2]| [f(x) * scale + o[0], f(y) * scale + o[1]];
            let (mut p0, mut p1, mut p2) = (at(t0[0], t0[1], oa), at(t0[2], t0[3], oa), at(t1[0], t1[1], ob));
            let (mut wlo, mut whi) = (-hw, hw);
            let (be0, be1) = (hy(e0 + oa[1]), hy(e1 + oa[1]));
            (p0[1], p1[1], p2[1]) = (hy(p0[1]), hy(p1[1]), hy(p2[1]));
            if code & super::build::CONNECTOR != 0 {
                p1 = [(p0[0] + p2[0]) * 0.5, (p0[1] + p2[1]) * 0.5];
            } else {
                wlo = wlo.max(be0 - local[1]);
                whi = whi.min(be1 - local[1]);
            }
            let rel = |p: [f32; 2]| [p[0] - local[0], p[1] - local[1]];
            let (q1, q2, q3) = (rel(p0), rel(p1), rel(p2));
            let lo = wlo.max(q1[1].min(q3[1]));
            let hi = whi.min(q1[1].max(q3[1]));
            if hi <= lo || q1[0].max(q3[0]) <= -hw {
                continue;
            }
            if q1[0].min(q3[0]) >= hw {
                acc += 2.0 * hw * (q3[1].clamp(wlo, whi) - q1[1].clamp(wlo, whi));
                continue;
            }
            acc += integrate(q1, q2, q3, lo, hi, hw);
        }
    }
    (acc.abs() / (4.0 * hw * hw)).min(1.0)
}

fn moved(code: u32, r: f32) -> [f32; 2] {
    [((code & 3) as f32 - 1.0) * r, ((code >> 2) & 3) as f32 * r]
}

fn hint_y(y: f32, a: [f32; 4], b: [f32; 4], c: [f32; 4]) -> f32 {
    if y < a[1] { return y; }
    if y <= a[2] { return (y - a[1]) * c[0] + a[1]; }
    if y <= b[0] { return (y - a[2]) * c[1] + a[3]; }
    if y <= b[2] { return y + (b[1] - b[0]); }
    (y - b[2]) * c[2] + b[3]
}

fn inside(a2: [f32; 2], a1: [f32; 2], x0: f32, ta: f32, tb: f32, hx: f32) -> f32 {
    if tb <= ta { return 0.0; }
    let tm = 0.5 * (ta + tb);
    let d = 0.5 * (tb - ta);
    let x_mid = (a2[0] * tm + a1[0]) * tm + x0 + hx;
    let dmid = [2.0 * a2[0] * tm + a1[0], 2.0 * a2[1] * tm + a1[1]];
    2.0 * d * x_mid * dmid[1] + (2.0 * d * d * d / 3.0) * (a2[0] * dmid[1] + 2.0 * a2[1] * dmid[0])
}

/// `outline_integrate`, its four lanes (y lo, y hi, x -hx, x +hx) written out.
fn integrate(q1: [f32; 2], q2: [f32; 2], q3: [f32; 2], lo: f32, hi: f32, hx: f32) -> f32 {
    let a2 = [q1[0] - 2.0 * q2[0] + q3[0], q1[1] - 2.0 * q2[1] + q3[1]];
    let a1 = [2.0 * (q2[0] - q1[0]), 2.0 * (q2[1] - q1[1])];
    let (y_rising, x_rising) = (q3[1] >= q1[1], q3[0] >= q1[0]);
    let lane = |k: usize| -> f32 {
        let (axis, rising) = if k < 2 { (1, y_rising) } else { (0, x_rising) };
        let v = match k { 0 => if y_rising { lo } else { hi }, 1 => if y_rising { hi } else { lo }, 2 => -hx, _ => hx };
        let (a2, a1) = (a2[axis], a1[axis]);
        let sg = if rising { 1.0 } else { -1.0 };
        let c = q1[axis] - v;
        let sat0 = c * sg >= 0.0;
        let sat1 = (q3[axis] - v) * sg <= 0.0;
        let sq = (a1 * a1 - 4.0 * a2 * c).max(0.0).sqrt();
        let qq = -0.5 * (a1 + if a1 >= 0.0 { sq } else { -sq });
        let use_r1 = (a1 < 0.0) == rising;
        let (num, den) = if use_r1 { (qq, a2) } else { (c, qq) };
        let t_raw = if den != 0.0 { (num / den).clamp(0.0, 1.0) } else { 0.0 };
        if sat0 { 0.0 } else if sat1 { 1.0 } else { t_raw }
    };
    let t = [lane(0), lane(1), lane(2), lane(3)];
    if t[1] <= t[0] { return 0.0; }
    let tc = [t[2].clamp(t[0], t[1]), t[3].clamp(t[0], t[1])];
    let t1 = if x_rising { tc[0] } else { tc[1] };
    let t2 = (if x_rising { tc[1] } else { tc[0] }).max(t1);
    let mut acc = inside(a2, a1, q1[0], t1, t2, hx);
    let rab = if x_rising { [t2, t[1]] } else { [t[0], t1] };
    let d = (rab[1] - rab[0]).max(0.0);
    let tm = 0.5 * (rab[0] + rab[1]);
    acc += d * (2.0 * a2[1] * tm + a1[1]) * (2.0 * hx);
    acc
}
