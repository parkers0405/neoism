//! Deterministic `exp`, `sin` and `cos`.
//!
//! Built only from IEEE-754 operations (`+ − × /`, `round`, and fused multiply-add where the
//! target has it), so the same inputs give bit-identical outputs on every machine of the same
//! kind: Apple silicon, aarch64 Linux and x86-64 with FMA (x86-64-v3) agree with each other;
//! x86-64 without FMA takes an equally accurate non-fused path. Platform `libm`s differ in the
//! last bits, which makes golden motion traces flaky across CI hosts (research 06 §1.7). These
//! are also several times faster than libm for the ranges a spring needs: polynomials are
//! evaluated with Estrin's scheme so a frame's lanes overlap in the pipeline.

#[cfg(any(target_arch = "aarch64", target_feature = "fma"))]
const FUSED: bool = true;
#[cfg(not(any(target_arch = "aarch64", target_feature = "fma")))]
const FUSED: bool = false;

/// `a·b + c`, fused (one rounding) where the hardware has it.
#[inline(always)]
fn fma(a: f32, b: f32, c: f32) -> f32 {
    if FUSED { a.mul_add(b, c) } else { a * b + c }
}

const LOG2_E: f32 = core::f32::consts::LOG2_E;
// fdlibm's split of ln 2: the high part has zero low bits so `k · LN2_HI` is exact for |k| < 2^12.
const LN2_HI: f32 = 6.931_457_5e-1;
const LN2_LO: f32 = 1.428_606_8e-6;

/// `e^x` with a relative error under 3e-7 for x in [−87, 88]. Below that it returns 0; above, ∞.
#[inline(always)]
pub fn exp(x: f32) -> f32 {
    let xc = x.clamp(-87.0, 88.0);
    let k = (xc * LOG2_E).round();
    let r = fma(-k, LN2_LO, fma(-k, LN2_HI, xc));
    // e^r on |r| ≤ ln2/2, Taylor to degree 6 (truncation 1.2e-7), Estrin's scheme.
    let r2 = r * r;
    let a = 1.0 + r;
    let b = fma(r, 1.0 / 6.0, 0.5);
    let c = fma(r, 1.0 / 120.0, 1.0 / 24.0);
    let p = fma(r2, fma(r2, fma(r2, 1.0 / 720.0, c), b), a);
    let bits = ((k as i32 + 127) as u32) << 23;
    let y = p * f32::from_bits(bits);
    if x < -87.0 { 0.0 } else if x > 88.0 { f32::INFINITY } else { y }
}

/// `(sin x, cos x)` with an absolute error under 3e-7 for |x| < 1e5.
#[inline(always)]
pub fn sin_cos(x: f32) -> (f32, f32) {
    let (q, r) = reduce_pio2(x);
    let z = r * r;
    let z2 = z * z;
    // Taylor on |r| ≤ π/4: sin to degree 9 (error 2e-9), cos to degree 10 (error 1e-10).
    let s_hi = fma(z, 1.0 / 120.0, -1.0 / 6.0);
    let s_lo = fma(z, 1.0 / 362_880.0, -1.0 / 5040.0);
    let s = fma(r * z, fma(z2, s_lo, s_hi), r);
    let c_hi = fma(z, 1.0 / 24.0, -0.5);
    let c_mid = fma(z, -1.0 / 3_628_800.0, 1.0 / 40_320.0);
    let c_lo = fma(z, c_mid, -1.0 / 720.0);
    let c = fma(z, fma(z2, c_lo, c_hi), 1.0);
    // Quadrant fix-up without branches (the quadrant is unpredictable across lanes):
    // q0 (s, c)  q1 (c, −s)  q2 (−s, −c)  q3 (−c, s).
    let swap = q & 1 != 0;
    let (sb, cb) = if swap { (c, s) } else { (s, c) };
    let sin_sign = (q & 2) << 30;
    let cos_sign = (q.wrapping_add(1) & 2) << 30;
    (f32::from_bits(sb.to_bits() ^ sin_sign), f32::from_bits(cb.to_bits() ^ cos_sign))
}

/// `x = q·π/2 + r` with |r| ≤ π/4; returns the low bits of q and r.
#[inline(always)]
#[allow(clippy::approx_constant)]
fn reduce_pio2(x: f32) -> (u32, f32) {
    if FUSED {
        // Two-part Cody–Waite in f32; exact enough with fused steps for |q| < 2^20.
        const HI: f32 = 1.570_796_4; // f32 nearest π/2
        const LO: f32 = -4.371_139e-8; // π/2 − HI
        let q = (x * core::f32::consts::FRAC_2_PI).round();
        let r = fma(-q, LO, fma(-q, HI, x));
        (q as i32 as u32, r)
    } else {
        // No FMA: reduce in f64, where the products are exact enough.
        const PIO2_HI: f64 = 1.570_796_326_734_125_6;
        const PIO2_LO: f64 = 6.077_100_506_506_192e-11;
        let xd = x as f64;
        let q = (xd * core::f64::consts::FRAC_2_PI).round();
        let r = ((xd - q * PIO2_HI) - q * PIO2_LO) as f32;
        (q as i64 as u32, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_matches_std() {
        let mut x = -87.0f32;
        while x < 88.0 {
            let ours = exp(x) as f64;
            let theirs = (x as f64).exp();
            assert!(((ours - theirs) / theirs).abs() < 4e-7, "exp({x}) = {ours}, std {theirs}");
            x += 0.0137;
        }
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(exp(-100.0), 0.0);
        assert_eq!(exp(100.0), f32::INFINITY);
    }

    #[test]
    fn sin_cos_match_std() {
        let mut x = -2000.0f32;
        while x < 2000.0 {
            let (s, c) = sin_cos(x);
            let xd = x as f64;
            assert!((s as f64 - xd.sin()).abs() < 4e-7, "sin({x}) {s} vs {}", xd.sin());
            assert!((c as f64 - xd.cos()).abs() < 4e-7, "cos({x}) {c} vs {}", xd.cos());
            x += 0.0731;
        }
    }
}
