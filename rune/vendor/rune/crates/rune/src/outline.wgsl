// The Windfoil variant of the uber shader (render/variant.rs): shader.wgsl with glyphs that
// draw from their outlines (`text::Outline`, `radii.w > 0`) as well as from the atlas.

// Glyph outlines (text/outline.rs): banded records of xy-monotone quadratics in font units.
@group(0) @binding(4) var outlines: texture_2d<u32>;

// Glyph coverage from outlines: Windfoil's closed-form box filter (DesLauriers 2026; the
// integrals are texel-org/windfoil-algorithm's, Apache-2.0) over a record's pieces, each moved
// by CoreGraphics' dilation and y autohint first (text/outline/build.rs has the record layout).

fn outline_texel(i: u32) -> vec4<u32> {
    return textureLoad(outlines, vec2<i32>(i32(i & 1023u), i32(i >> 10u)), 0);
}

// The move of a piece's code in radii: x in -1..1, y in 0..2.
fn outline_move(code: u32) -> vec2<f32> {
    return vec2<f32>(f32(code & 3u) - 1.0, f32((code >> 2u) & 3u));
}

// CoreGraphics' y autohint (`HintTransform::apply`): values [-d, lo, x, xt] [middle, mt, cap, ct]
// [s1, s2, s3].
fn outline_hint(y: f32, a: vec4<f32>, b: vec4<f32>, c: vec4<f32>) -> f32 {
    if (y < a.y) { return y; }
    if (y <= a.z) { return (y - a.y) * c.x + a.y; }
    if (y <= b.x) { return (y - a.z) * c.y + a.w; }
    if (y <= b.z) { return y + (b.y - b.x); }
    return (y - b.z) * c.z + b.w;
}

fn outline_inside(a2: vec2<f32>, a1: vec2<f32>, x0: f32, ta: f32, tb: f32, hx: f32) -> f32 {
    if (tb <= ta) { return 0.0; }
    let tm = 0.5 * (ta + tb);
    let d = 0.5 * (tb - ta);
    let x_mid = (a2.x * tm + a1.x) * tm + x0 + hx;
    let dmid = 2.0 * a2 * tm + a1;
    return 2.0 * d * x_mid * dmid.y + (2.0 * d * d * d / 3.0) * (a2.x * dmid.y + 2.0 * a2.y * dmid.x);
}

// The box-filtered winding of one monotone piece (relative to the pixel centre) between rows
// `lo` and `hi`, `2 * hx` wide.
fn outline_integrate(q1: vec2<f32>, q2: vec2<f32>, q3: vec2<f32>, lo: f32, hi: f32, hx: f32) -> f32 {
    let a2 = q1 - 2.0 * q2 + q3;
    let a1 = 2.0 * (q2 - q1);
    let y_rising = q3.y >= q1.y;
    let x_rising = q3.x >= q1.x;
    let A2 = a2.yyxx;
    let A1 = a1.yyxx;
    let V = vec4<f32>(select(vec2<f32>(hi, lo), vec2<f32>(lo, hi), y_rising), -hx, hx);
    let R = vec2<bool>(y_rising, x_rising).xxyy;
    let SG = select(vec4<f32>(-1.0), vec4<f32>(1.0), R);
    let C = q1.yyxx - V;
    let sat0 = (C * SG) >= vec4<f32>(0.0);
    let sat1 = ((q3.yyxx - V) * SG) <= vec4<f32>(0.0);
    let disc = max(A1 * A1 - 4.0 * A2 * C, vec4<f32>(0.0));
    let sq = sqrt(disc);
    let qq = -0.5 * (A1 + select(-sq, sq, A1 >= vec4<f32>(0.0)));
    let use_r1 = (A1 < vec4<f32>(0.0)) == R;
    let num = select(C, qq, use_r1);
    let den = select(qq, A2, use_r1);
    let valid = den != vec4<f32>(0.0);
    let t_raw = clamp(select(vec4<f32>(0.0), num / select(vec4<f32>(1.0), den, valid), valid), vec4<f32>(0.0), vec4<f32>(1.0));
    let T = select(select(t_raw, vec4<f32>(1.0), sat1), vec4<f32>(0.0), sat0);
    if (T.y <= T.x) { return 0.0; }
    let tc = clamp(T.zw, T.xx, T.yy);
    let t1 = select(tc.y, tc.x, x_rising);
    let t2 = max(select(tc.x, tc.y, x_rising), t1);
    var acc = outline_inside(a2, a1, q1.x, t1, t2, hx);
    let rab = select(vec2<f32>(T.x, t1), vec2<f32>(t2, T.y), x_rising);
    let d = max(rab.y - rab.x, 0.0);
    let tm = 0.5 * (rab.x + rab.y);
    acc = acc + d * (2.0 * a2.y * tm + a1.y) * (2.0 * hx);
    return acc;
}

// How much of a pixel `2 * hw` rest px square at `local` (rest px from the pen, y up) the glyph
// of record `rec` covers, `scale` rest px a font unit, dilated by `r` rest px and hinted by
// record `hint` (0: none).
fn outline_coverage(rec: u32, hint: u32, local: vec2<f32>, scale: f32, r: f32, hw: f32) -> f32 {
    let head = outline_texel(rec);
    let count = head.y;
    let y0 = bitcast<f32>(head.z);
    let inv_h = bitcast<f32>(head.w);
    let hinted = hint != 0u;
    var ha = vec4<f32>(0.0);
    var hb = vec4<f32>(0.0);
    var hc = vec4<f32>(0.0);
    if (hinted) {
        ha = bitcast<vec4<f32>>(outline_texel(hint));
        hb = bitcast<vec4<f32>>(outline_texel(hint + 1u));
        hc = bitcast<vec4<f32>>(outline_texel(hint + 2u));
    }
    // Pieces from as far as 2r below move up into this row; the hint moves them up to its reach.
    let reach = select(0.0, OUTLINE_HINT_REACH, hinted);
    var b0 = 0u;
    var b1 = 0u;
    if (count > 1u) {
        let last = f32(count - 1u);
        b0 = u32(clamp(floor(((local.y - hw - 2.0 * r - reach) / scale - y0) * inv_h), 0.0, last));
        b1 = u32(clamp(floor(((local.y + hw + reach) / scale - y0) * inv_h), 0.0, last));
    }
    var acc = 0.0;
    for (var band = b0; band <= b1; band = band + 1u) {
        // A piece counts within its band's rows, moved as it moves; a connector lies in one band.
        var e0 = -3.0e38;
        var e1 = 3.0e38;
        if (count > 1u) {
            e0 = (y0 + f32(band) / inv_h) * scale;
            e1 = (y0 + f32(band + 1u) / inv_h) * scale;
        }
        let entry = outline_texel(head.x + band);
        let sorted = entry.y > 4u;
        for (var k = 0u; k < entry.y; k = k + 1u) {
            let t0 = outline_texel(entry.x + 2u * k);
            let t1 = outline_texel(entry.x + 2u * k + 1u);
            // Sorted by right edge: once one sits left of the pixel, all the rest do.
            if (bitcast<f32>(t1.w) * scale + r - local.x <= -hw) {
                if (sorted) { break; }
                continue;
            }
            let code = t1.z;
            let oa = outline_move(code & 15u) * r;
            let ob = outline_move((code >> 4u) & 15u) * r;
            var p0 = bitcast<vec2<f32>>(t0.xy) * scale + oa;
            var p1 = bitcast<vec2<f32>>(t0.zw) * scale + oa;
            var p2 = bitcast<vec2<f32>>(t1.xy) * scale + ob;
            var wlo = -hw;
            var whi = hw;
            var be0 = e0 + oa.y;
            var be1 = e1 + oa.y;
            if (hinted) {
                p0.y = outline_hint(p0.y, ha, hb, hc);
                p1.y = outline_hint(p1.y, ha, hb, hc);
                p2.y = outline_hint(p2.y, ha, hb, hc);
                be0 = outline_hint(be0, ha, hb, hc);
                be1 = outline_hint(be1, ha, hb, hc);
            }
            if ((code & 256u) != 0u) {
                p1 = (p0 + p2) * 0.5;
            } else {
                wlo = max(wlo, be0 - local.y);
                whi = min(whi, be1 - local.y);
            }
            let q1 = p0 - local;
            let q3 = p2 - local;
            let lo = max(wlo, min(q1.y, q3.y));
            let hi = min(whi, max(q1.y, q3.y));
            if (hi <= lo || max(q1.x, q3.x) <= -hw) { continue; }
            if (min(q1.x, q3.x) >= hw) {
                acc = acc + 2.0 * hw * (clamp(q3.y, wlo, whi) - clamp(q1.y, wlo, whi));
                continue;
            }
            acc = acc + outline_integrate(q1, p1 - local, q3, lo, hi, hw);
        }
    }
    return min(abs(acc) / (4.0 * hw * hw), 1.0);
}

const OUTLINE_HINT_REACH: f32 = 1.5;

// A glyph instance drawn from its outline (`text::Outline`): radii = pen, scale, zoom; border =
// record, hint, radius. Rest px, y up, a device pixel 1 / zoom of them across.
fn outline_glyph(i: Out, screen: vec2<f32>) -> f32 {
    let z = i.radii.w;
    let local = vec2<f32>(screen.x - i.radii.x, i.radii.y - screen.y) / z;
    return outline_coverage(u32(i.border.x), u32(i.border.y), local, i.radii.z, i.border.z, 0.5 / z);
}
