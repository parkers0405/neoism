// One instanced "uber-quad": rounded rects with per-corner radius and border (kind 0), analytic
// Gaussian rounded-rect shadows (kind 1), glyph-atlas text (kind 2), inset shadows (kind 3),
// vertical-gradient rounded rects or borders (kind 4), stroke segments (kind 5) and filled
// triangles (kind 6) for vector shapes, RGBA images (kind 7), and premultiplied layer composites
// (kind 8), dashed borders (kind 10), and radial or angled linear gradients (kind 11).
// Scope-end records (kind 9) never reach this shader. Every ordinary instance carries a
// soft-edge clip. Output is premultiplied linear colour into an sRGB target (or sRGB into a
// non-sRGB target when `srgb_blend` is enabled).

struct Globals {
    viewport: vec2<f32>,
    pad: vec2<f32>,
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var images: texture_2d<f32>;
// Zero size selects the window viewport. Layer targets keep all geometry/clip coordinates
// absolute and change only the final projection, including GPU-spring translations.
@group(0) @binding(3) var<uniform> projection: vec4<f32>;

struct In {
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) radii: vec4<f32>,
    @location(2) fill: vec4<f32>,
    @location(3) border: vec4<f32>,
    @location(4) params: vec4<f32>,
    @location(5) uv: vec4<f32>,
    @location(6) clip: vec4<f32>,
    @location(7) fade: vec4<u32>,
};

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) radii: vec4<f32>,
    @location(3) @interpolate(flat) fill: vec4<f32>,
    @location(4) @interpolate(flat) border: vec4<f32>,
    @location(5) @interpolate(flat) params: vec4<f32>,
    @location(6) @interpolate(flat) uv: vec4<f32>,
    @location(7) @interpolate(flat) clip: vec4<f32>,
    @location(8) @interpolate(flat) fade: vec4<u32>,
};

// Layer composites use `vs` even with GPU springs: the layer's contents already sampled the
// retained instance tracks using their original absolute instance indices.
@vertex
fn vs(i: In) -> Out {
    return base_vs(i);
}

fn base_vs(i: In) -> Out {
    let corner = vec2<f32>(f32(i.vi & 1u), f32(i.vi >> 1u));
    let kind = i.params.y;
    var grow = 1.0 + i.params.z * 2.0;
    if (kind > 0.5 && kind < 1.5) {
        grow = i.params.z * 3.0 + 1.0;
    } else if (kind > 1.5 && kind < 2.5) {
        grow = 0.0;
    } else if (kind > 2.5 && kind < 3.5) {
        grow = 1.0;
    } else if (kind > 4.5 && kind < 9.5) {
        grow = 1.0;
    }
    var lo = i.rect.xy - vec2<f32>(grow);
    // Effect slots (kind 12) reserve their place in the stream and cover no pixels.
    let size = select(i.rect.zw + vec2<f32>(2.0 * grow), vec2<f32>(0.0), kind > 11.5);
    var hi = lo + size;
    // One band of a big shadow or border ring rasterizes only its window (runtime/bands.rs).
    // Neighbouring bands compute a shared edge the same way, so they meet without a seam.
    if (kind < 1.5 && i.uv.z < 0.0) {
        lo = max(lo, i.rect.xy + i.uv.xy);
        hi = max(min(hi, i.rect.xy + vec2<f32>(-i.uv.z, i.uv.w)), lo);
    }
    let p = select(lo, hi, corner > vec2<f32>(0.5));
    var screen = p;
    // Icon loops rotate the quad, not its atlas allocation. Local p still samples the glyph.
    if (kind > 1.5 && kind < 2.5 && i.params.x != 0.0) {
        let center = i.rect.xy + i.rect.zw * 0.5;
        let q = p - center;
        let c = cos(i.params.x);
        let s = sin(i.params.x);
        screen = center + vec2<f32>(c * q.x - s * q.y, s * q.x + c * q.y);
    }
    // A turned layer composite (`Instance::turned`): border.y radians about border.zw.
    if (kind > 7.5 && kind < 8.5 && i.border.y != 0.0) {
        let q = p - i.border.zw;
        let c = cos(i.border.y);
        let s = sin(i.border.y);
        screen = i.border.zw + vec2<f32>(c * q.x - s * q.y, s * q.x + c * q.y);
    }
    var o: Out;
    let size_px = select(g.viewport, projection.zw, projection.z > 0.0);
    let local = screen - projection.xy;
    o.pos = vec4<f32>(local.x / size_px.x * 2.0 - 1.0, 1.0 - local.y / size_px.y * 2.0, 0.0, 1.0);
    o.p = p;
    o.rect = i.rect;
    o.radii = i.radii;
    o.fill = i.fill;
    o.border = i.border;
    o.params = i.params;
    o.uv = i.uv;
    o.clip = i.clip;
    o.fade = i.fade;
    return o;
}

// Evan Wallace's analytic rounded-box shadow (as in GPUI): exact along x, 4 samples along y.
fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * 3.14159265) * sigma);
}

fn erf2(x: vec2<f32>) -> vec2<f32> {
    let s = sign(x);
    let a = abs(x);
    var r = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    r = r * r;
    return s - s / (r * r);
}

fn shadow_x(x: f32, y: f32, sigma: f32, corner: f32, half: vec2<f32>) -> f32 {
    let delta = min(half.y - corner - abs(y), 0.0);
    let curved = half.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = 0.5 + 0.5 * erf2((x + vec2<f32>(-curved, curved)) * (sqrt(0.5) / sigma));
    return integral.y - integral.x;
}

// What `shadow` sums to well inside the box, where every row it samples is fully covered: the
// four midpoint weights of the ±3σ window, 3·(g(0.75) + g(2.25)) for a unit Gaussian g.
const SHADOW_INSIDE: f32 = 0.99863125;

fn shadow(p: vec2<f32>, half: vec2<f32>, sigma: f32, corner: f32) -> f32 {
    // A big panel's shadow is mostly interior, and its erf rows there cost a 5K frame
    // milliseconds. 6σ in, each row is within 1e-6 of fully covered.
    if (abs(p.x) <= half.x - 6.0 * sigma && abs(p.y) <= half.y - corner - 6.0 * sigma) {
        return SHADOW_INSIDE;
    }
    let lo = p.y - half.y;
    let hi = p.y + half.y;
    let start = clamp(-3.0 * sigma, lo, hi);
    let end = clamp(3.0 * sigma, lo, hi);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var v = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        v = v + shadow_x(p.x, p.y - y, sigma, corner, half) * gaussian(y, sigma) * step;
        y = y + step;
    }
    return v;
}

// Coverage of a stroke segment from a to b, `hw` half width, round or butt caps. Strokes
// thinner than a pixel draw a pixel wide at proportionally lower coverage.
fn segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, hw: f32, round: bool) -> f32 {
    let w = max(hw, 0.5);
    let weight = min(hw * 2.0, 1.0);
    let ab = b - a;
    let len = length(ab);
    let ap = p - a;
    var d: f32;
    if (len < 1e-4) {
        if (!round) {
            return 0.0;
        }
        d = length(ap) - w;
    } else {
        let dir = ab / len;
        let along = dot(ap, dir);
        if (round) {
            d = length(ap - dir * clamp(along, 0.0, len)) - w;
        } else {
            let across = abs(dir.x * ap.y - dir.y * ap.x);
            let q = vec2<f32>(abs(along - len * 0.5) - len * 0.5, across - w);
            d = min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0)));
        }
    }
    return clamp(0.5 - d, 0.0, 1.0) * weight;
}

// Signed distance from p to the edge a->b, positive on the side `c` lies on.
fn edge(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> f32 {
    let e = b - a;
    let l = max(length(e), 1e-6);
    let n = vec2<f32>(-e.y, e.x) / l;
    let s = select(-1.0, 1.0, dot(c - a, n) >= 0.0);
    return dot(p - a, n) * s;
}

// Whether p is on c's side of the edge a-b, decided so that the two triangles sharing the edge
// compute the same value with opposite signs and exactly one of them owns every pixel: the
// endpoints are put in a fixed order and ties go to the positive side.
fn owns(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> bool {
    let swap = a.x > b.x || (a.x == b.x && a.y > b.y);
    let lo = select(a, b, swap);
    let hi = select(b, a, swap);
    let e = hi - lo;
    let side = e.x * (c.y - lo.y) - e.y * (c.x - lo.x);
    let v = e.x * (p.y - lo.y) - e.y * (p.x - lo.x);
    if (side > 0.0) {
        return v >= 0.0;
    }
    return v < 0.0;
}

// Coverage across a polygon edge a->b. Only the edge itself is antialiased: past its ends
// (where a concave polygon's interior continues along the edge's line) the edge cuts hard, so
// the neighbouring triangle that owns those pixels is not shaded through.
fn soft(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> f32 {
    let d = edge(p, a, b, c);
    let e = b - a;
    let t = dot(p - a, e) / max(dot(e, e), 1e-6);
    if (t < 0.0 || t > 1.0) {
        return select(0.0, 1.0, d >= 0.0);
    }
    return clamp(d + 0.5, 0.0, 1.0);
}

// Coverage of a filled triangle; `aa` marks the polygon-boundary edges, which are antialiased
// (bits 0, 1, 2 for a->b, b->c, c->a). Other edges are shared with a neighbouring triangle and
// cut hard, with exactly one of the two owning each pixel.
fn triangle(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>, aa: u32) -> f32 {
    var cov = 1.0;
    if ((aa & 1u) == 1u) {
        cov = cov * soft(p, a, b, c);
    } else if (!owns(p, a, b, c)) {
        return 0.0;
    }
    if ((aa & 2u) == 2u) {
        cov = cov * soft(p, b, c, a);
    } else if (!owns(p, b, c, a)) {
        return 0.0;
    }
    if ((aa & 4u) == 4u) {
        cov = cov * soft(p, c, a, b);
    } else if (!owns(p, c, a, b)) {
        return 0.0;
    }
    return cov;
}

fn srgb_encode(x: vec3<f32>) -> vec3<f32> {
    let v = clamp(x, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(v, vec3<f32>(1.0 / 2.4)) - 0.055, v * 12.92, v <= vec3<f32>(0.0031308));
}

/// One image texel, premultiplied (the atlas holds straight alpha) so filtering doesn't darken edges.
fn image_texel(t: vec2<i32>, premultiplied: bool) -> vec4<f32> {
    let c = textureLoad(images, t, 0);
    if (g.flags.x > 0.5) {
        if (c.a <= 0.0) { return vec4<f32>(0.0); }
        let lin = select(c.rgb, c.rgb / c.a, premultiplied);
        return vec4<f32>(srgb_encode(lin) * c.a, c.a);
    }
    if premultiplied { return c; }
    return vec4<f32>(c.rgb * c.a, c.a);
}

// Arc length to the point nearest `p` (rect-local) on the centre line of a `bw`-wide ring inside
// the rounded rect, and the line's full length. It runs clockwise from where the top edge leaves
// the top-left corner; corners count along their arcs.
fn ring_arc(p: vec2<f32>, size: vec2<f32>, radii: vec4<f32>, bw: f32) -> vec2<f32> {
    let h = bw * 0.5;
    let half = size * 0.5;
    let rc = max(min(abs(radii), vec4<f32>(min(half.x, half.y))) - vec4<f32>(h), vec4<f32>(0.0));
    let e = rc + vec4<f32>(h);
    let arc = rc * 1.5707964;
    let s_tr = max(size.x - bw - rc.x - rc.y, 0.0);
    let s_right = s_tr + arc.y;
    let s_br = s_right + max(size.y - bw - rc.y - rc.z, 0.0);
    let s_bottom = s_br + arc.z;
    let s_bl = s_bottom + max(size.x - bw - rc.z - rc.w, 0.0);
    let s_left = s_bl + arc.w;
    let s_tl = s_left + max(size.y - bw - rc.w - rc.x, 0.0);
    let top = p.x - e.x;
    let right = s_right + p.y - e.y;
    let bottom = s_bottom + size.x - e.z - p.x;
    let left = s_left + size.y - e.w - p.y;
    var s: f32;
    if (p.x > half.x) {
        if (p.y > half.y) {
            let v = p - (size - vec2<f32>(e.z));
            if (v.x > 0.0 && v.y > 0.0) { s = s_br + atan2(v.y, v.x) * rc.z; }
            else { s = select(bottom, right, size.x - p.x < size.y - p.y); }
        } else {
            let v = p - vec2<f32>(size.x - e.y, e.y);
            if (v.x > 0.0 && v.y < 0.0) { s = s_tr + atan2(v.x, -v.y) * rc.y; }
            else { s = select(top, right, size.x - p.x < p.y); }
        }
    } else if (p.y > half.y) {
        let v = p - vec2<f32>(e.w, size.y - e.w);
        if (v.x < 0.0 && v.y > 0.0) { s = s_bl + atan2(-v.x, v.y) * rc.w; }
        else { s = select(bottom, left, p.x < size.y - p.y); }
    } else {
        let v = p - vec2<f32>(e.x);
        if (v.x < 0.0 && v.y < 0.0) { s = s_tl + atan2(-v.y, -v.x) * rc.x; }
        else { s = select(top, left, p.x < p.y); }
    }
    return vec2<f32>(s, s_tl + arc.x);
}

// Coverage of a dash pattern (dash, gap, phase) at arc length `s`. The period stretches or
// shrinks so a whole number of periods fills `total`, and the pattern meets itself at the seam.
fn dash_cov(s: f32, total: f32, d: vec3<f32>) -> f32 {
    let nominal = d.x + d.y;
    let k = max(total, 0.001) / (max(floor(total / nominal + 0.5), 1.0) * nominal);
    let period = nominal * k;
    let dash = d.x * k;
    let a = s + d.z * k - dash * 0.5;
    let m = a - period * floor(a / period + 0.5);
    return clamp(dash * 0.5 - abs(m) + 0.5, 0.0, 1.0);
}

// A border over its own background, as CSS paints one: the background runs under the border to
// the outer edge (`background-clip: border-box`) and the ring of `edge` draws over it. `d` and
// `inner_d` are the distances to the outer and inner edges. `opacity` is what the colours were
// already multiplied by, so `edge.a / opacity` is how much the border hides the fill. An opaque
// border all round a rounded box (`shrink`) pulls the fill in by half its width, as Chromium's
// bleed avoidance does, so no fill fringes the anti-aliased outer edge.
fn border_over(fill: vec4<f32>, edge: vec4<f32>, d: f32, inner_d: f32, soft: f32, bw: f32, opacity: f32, shrink: bool) -> vec4<f32> {
    let cov = clamp(0.5 - d / (soft * 2.0), 0.0, 1.0);
    let ring = max(cov - clamp(0.5 - inner_d / (soft * 2.0), 0.0, 1.0), 0.0);
    let hide = clamp(edge.a / clamp(opacity, 1e-6, 1.0), 0.0, 1.0);
    let under = clamp(0.5 - (d + select(0.0, bw * 0.5, shrink && hide > 0.999)) / (soft * 2.0), 0.0, 1.0);
    return edge * ring + fill * (under * (1.0 - hide * ring));
}

// How much of the pixel at `p` the box covers. Where the corner is square that's the exact
// area, as Chromium rasterises a rect; a rounded corner takes the distance estimate.
fn box_cov(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
    var r = radii.x;
    if (p.x > 0.0) {
        r = select(radii.y, radii.z, p.y > 0.0);
    } else {
        r = select(radii.x, radii.w, p.y > 0.0);
    }
    if (r == 0.0) {
        let e = clamp(min(half, p + vec2<f32>(0.5)) - max(-half, p - vec2<f32>(0.5)), vec2<f32>(0.0), vec2<f32>(1.0));
        return e.x * e.y;
    }
    return clamp(0.5 - sd_rounded(p, half, radii), 0.0, 1.0);
}

// A layer composite's round hole (`El::cutout_circle`): centre `h.xy`, clear within `h.z`, whole
// from `h.w`, linear between and at least a pixel wide; none when `h.w` is zero.
fn hole_alpha(p: vec2<f32>, h: vec4<f32>) -> f32 {
    if (h.w <= 0.0) { return 1.0; }
    return clamp((length(p - h.xy) - (h.z + h.w) * 0.5) / max(h.w - h.z, 1.0) + 0.5, 0.0, 1.0);
}

// A texel of a layer `size` pixels across; transparent outside it.
fn layer_texel(t: vec2<i32>, size: vec2<f32>) -> vec4<f32> {
    if (any(t < vec2<i32>(0)) || any(t >= vec2<i32>(size))) { return vec4<f32>(0.0); }
    return textureLoad(images, t, 0);
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    if (i.params.y == 8.0) {
        // Filters consume unmasked source pixels. Decode the same packed clip as primitives
        // only here, on the completed layer, including the radius in word 3.
        let screen = i.pos.xy + projection.xy;
        var p = screen - i.rect.xy;
        var color: vec4<f32>;
        if (i.border.y != 0.0) {
            // Turned: sample the upright contents where this pixel turns back to, bilinearly,
            // with nothing outside the layer.
            let q = screen - i.border.zw;
            let c = cos(i.border.y);
            let s = sin(i.border.y);
            p = i.border.zw + vec2<f32>(c * q.x + s * q.y, c * q.y - s * q.x) - i.rect.xy;
            let local = p - vec2<f32>(0.5);
            let base = floor(local);
            let f = local - base;
            let t0 = vec2<i32>(base);
            color = mix(mix(layer_texel(t0, i.rect.zw), layer_texel(t0 + vec2<i32>(1, 0), i.rect.zw), f.x),
                        mix(layer_texel(t0 + vec2<i32>(0, 1), i.rect.zw), layer_texel(t0 + vec2<i32>(1, 1), i.rect.zw), f.x), f.y);
        } else if (any(p < vec2<f32>(0.0)) || any(p >= i.rect.zw)) {
            discard;
        } else if (i.uv.z > 0.0) {
            let local = p * i.uv.zw / i.rect.zw - vec2<f32>(0.5);
            let hi = vec2<i32>(i.uv.zw) - vec2<i32>(1);
            let base = floor(local);
            let f = local - base;
            let t0 = clamp(vec2<i32>(base), vec2<i32>(0), hi);
            let t1 = clamp(vec2<i32>(base) + vec2<i32>(1, 1), vec2<i32>(0), hi);
            color = mix(mix(textureLoad(images, t0, 0), textureLoad(images, vec2<i32>(t1.x, t0.y), 0), f.x),
                        mix(textureLoad(images, vec2<i32>(t0.x, t1.y), 0), textureLoad(images, t1, 0), f.x), f.y);
        } else {
            color = textureLoad(images, vec2<i32>(floor(p)), 0);
        }
        // Float intermediates accumulate One/One without saturation. Clamp before applying
        // this scope's opacity so nesting preserves plus-lighter's bounded coverage.
        return select(color, clamp(color, vec4(0.0), vec4(1.0)), i.params.x == 1.0) * i.fill.a
            * clip_alpha(screen, i.clip, i.fade) * hole_alpha(screen, i.radii);
    }
    let kind = i.params.y;
    if (kind > 11.5) { discard; }
    let half = i.rect.zw * 0.5;
    let center = i.rect.xy + half;
    var col = vec4<f32>(0.0);
    var clip_p = i.p;
    if (kind < 0.5) {
        let d = sd_rounded(i.p - center, half, i.radii);
        let soft = max(i.params.z, 0.5);
        let bw = i.params.x;
        if (bw > 0.0) {
            // Per-side widths (top, right, bottom, left) in `uv`, as CSS `border-bottom` and the
            // like; all zero is `bw` all round, and a band window (runtime/bands.rs) has a
            // negative `uv.z`.
            let w = i.uv;
            let sides = w.z >= 0.0 && any(w != vec4<f32>(0.0));
            var inner_d = d + bw;
            if (sides) {
                let ih = max(half - vec2<f32>(w.y + w.w, w.x + w.z) * 0.5, vec2<f32>(0.0));
                let ic = center + vec2<f32>(w.w - w.y, w.x - w.z) * 0.5;
                let cut = vec4<f32>(max(w.x, w.w), max(w.x, w.y), max(w.z, w.y), max(w.z, w.w));
                inner_d = sd_rounded(i.p - ic, ih, sign(i.radii) * max(abs(i.radii) - cut, vec4<f32>(0.0)));
            }
            col = border_over(i.fill, i.border, d, inner_d, soft, bw, i.params.w, !sides && any(i.radii != vec4<f32>(0.0)));
        } else {
            col = i.fill * clamp(0.5 - d / (soft * 2.0), 0.0, 1.0);
        }
    } else if (kind < 1.5) {
        let sigma = max(i.params.z, 0.01);
        let corner = min(abs(i.radii.x), min(half.x, half.y));
        col = i.fill * shadow(i.p - center, half, sigma, corner);
        // A shadow under a native system material leaves its interior clear: `uv` holds the
        // hole relative to this rect and `radii.w` its corner radius.
        if (i.uv.z > 0.0) {
            let hole = i.uv.zw * 0.5;
            col = col * clamp(0.5 + sd_rounded(i.p - (i.rect.xy + i.uv.xy + hole), hole, vec4<f32>(i.radii.w)), 0.0, 1.0);
        }
    } else if (kind > 6.5) {
        let d = sd_rounded(i.p - center, half, i.radii);
        if (kind > 10.5) {
            // Radial (`uv.z > 0`) or angled linear (`uv.z == 0`) gradient stop segment from
            // `fill` at t0 (`params.x`) to `border` at t1 (`params.w`); `+ 2.0` marks an open
            // interior stop boundary so adjacent segments partition the quad without overlap.
            let soft = max(i.params.z, 0.5);
            let cov = clamp(0.5 - d / (soft * 2.0), 0.0, 1.0);
            let t0_raw = i.params.x;
            let t1_raw = i.params.w;
            let lower_open = t0_raw >= 1.5;
            let upper_open = t1_raw >= 1.5;
            let t0 = select(t0_raw, t0_raw - 2.0, lower_open);
            let t1 = select(t1_raw, t1_raw - 2.0, upper_open);
            var t: f32;
            if (i.uv.z > 0.0) {
                t = length((i.p - i.rect.xy - i.uv.xy) / i.uv.zw);
            } else {
                t = 0.5 + dot(i.p - center, i.uv.xy);
            }
            if ((lower_open && t < t0) || (upper_open && t >= t1)) {
                col = vec4<f32>(0.0);
            } else {
                let u = select(clamp((t - t0) / (t1 - t0), 0.0, 1.0), select(0.0, 1.0, t >= t0), t1 <= t0 + 1e-6);
                col = mix(i.fill, i.border, u) * cov;
            }
        } else if (kind > 9.5) {
            // Dashed border: kind 0's ring, colour running from `fill` (top) to `border`
            // (bottom), masked by the dash pattern in `uv.xyz` along the ring's centre line.
            let soft = max(i.params.z, 0.5);
            let bw = i.params.x;
            let ring = clamp(0.5 - d / (soft * 2.0), 0.0, 1.0) - clamp(0.5 - (d + bw) / (soft * 2.0), 0.0, 1.0);
            let t = clamp((i.p.y - i.rect.y) / max(i.rect.w, 1.0), 0.0, 1.0);
            // The pixel centre, not `p`: inside a tight corner s jumps across the diagonal, and
            // interpolation could put a pixel on that diagonal on the other side of it.
            let arc = ring_arc(i.pos.xy + projection.xy - i.rect.xy, i.rect.zw, i.radii, bw);
            col = mix(i.fill, i.border, t) * (ring * dash_cov(arc.x, arc.y, i.uv.xyz));
        } else {
            // Bilinear, clamped to the image's own texels so neighbours in the atlas never bleed.
            let local = (i.p - i.rect.xy) * i.uv.zw / i.rect.zw - vec2<f32>(0.5);
            let lo = vec2<i32>(i.uv.xy);
            let hi = lo + vec2<i32>(i.uv.zw) - vec2<i32>(1);
            let base = floor(local);
            let f = local - base;
            let t0 = clamp(lo + vec2<i32>(base), lo, hi);
            let t1 = clamp(lo + vec2<i32>(base) + vec2<i32>(1, 1), lo, hi);
            let premultiplied = i.params.z > 0.5;
            let c = mix(mix(image_texel(t0, premultiplied), image_texel(vec2<i32>(t1.x, t0.y), premultiplied), f.x),
                        mix(image_texel(vec2<i32>(t0.x, t1.y), premultiplied), image_texel(t1, premultiplied), f.x), f.y);
            col = c * (i.fill.a * clamp(0.5 - d, 0.0, 1.0));
        }
    } else if (kind > 4.5 && kind < 5.5) {
        col = i.fill * segment(i.pos.xy + projection.xy, i.radii.xy, i.radii.zw, i.params.x, i.params.z > 0.5);
    } else if (kind > 5.5 && kind < 6.5) {
        // The pixel centre itself, not `p`: `p` is interpolated over each triangle's own quad, so
        // neighbours would disagree in the last bits about who owns a pixel on a shared edge.
        col = i.fill * triangle(i.pos.xy + projection.xy, i.radii.xy, i.radii.zw, i.uv.xy, u32(i.params.x + 0.5));
    } else if (kind > 2.5) {
        if (kind < 3.5) {
            // Inset shadow: the element's shape minus a blurred copy of it shrunk by the spread
            // and moved by the offset. Square corners take exact pixel areas, so a hairline
            // ring's corner pixel is as dark as Chromium draws it.
            let spread = i.params.x;
            let ih = max(half - vec2<f32>(spread), vec2<f32>(0.0));
            let q = i.p - center - i.uv.xy;
            var lit: f32;
            if (i.params.z < 0.3) {
                lit = box_cov(q, ih, sign(i.radii) * max(abs(i.radii) - vec4<f32>(spread), vec4<f32>(0.0)));
            } else {
                lit = shadow(q, ih, i.params.z, max(min(abs(i.radii.x) - spread, min(ih.x, ih.y)), 0.0));
            }
            col = i.fill * box_cov(i.p - center, half, i.radii) * (1.0 - clamp(lit, 0.0, 1.0));
        } else {
            // With a border width, kind 0 whose ring runs from `border` (top) to `uv` (bottom).
            let d = sd_rounded(i.p - center, half, i.radii);
            let t = clamp((i.p.y - i.rect.y) / max(i.rect.w, 1.0), 0.0, 1.0);
            let soft = max(i.params.z, 0.5);
            let bw = i.params.x;
            if (bw > 0.0) {
                col = border_over(i.fill, mix(i.border, i.uv, t), d, d + bw, soft, bw, i.params.w, any(i.radii != vec4<f32>(0.0)));
            } else {
                col = mix(i.fill, i.uv, t) * clamp(0.5 - d / (soft * 2.0), 0.0, 1.0);
            }
        }
    } else {
        // Interpolated screen coordinates can round below an integer texel
        // boundary. Fragment position is the rasterizer's exact pixel center.
        let screen = i.pos.xy + projection.xy;
        let rotated = i.params.x != 0.0;
        // A turned glyph clips on screen, where its quad is.
        if rotated { clip_p = screen; }
        var cov: f32;
        if i.uv.z > 0.0 || rotated {
            // Flying text keeps the original atlas footprint while its quad scales uniformly.
            // A turned icon samples at its unturned point, filtered: no texel lines up with a pixel.
            let p = select(screen, i.p, rotated);
            let span = select(i.rect.zw, i.uv.zw, i.uv.z > 0.0);
            let local = (p - i.rect.xy) * span / i.rect.zw - vec2<f32>(0.5);
            let uv = vec2<i32>(i.uv.xy + floor(local));
            let f = fract(local);
            cov = mix(mix(textureLoad(atlas, uv, 0).r, textureLoad(atlas, uv + vec2<i32>(1, 0), 0).r, f.x),
                      mix(textureLoad(atlas, uv + vec2<i32>(0, 1), 0).r, textureLoad(atlas, uv + vec2<i32>(1, 1), 0).r, f.x), f.y);
        } else {
            let texel = vec2<i32>(i.uv.xy + floor(screen - i.rect.xy));
            cov = textureLoad(atlas, texel, 0).r;
        }
        col = i.fill * pow(cov, i.params.w);
    }
    return col * clip_alpha(clip_p, i.clip, i.fade);
}
