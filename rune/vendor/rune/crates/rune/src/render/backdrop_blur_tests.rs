use super::{SIGMAS, levels};

#[test]
fn levels_cover_the_range_continuously() {
    let top = SIGMAS.len() - 1;
    assert_eq!(levels(0.0, top), (1, 0.0));
    assert_eq!(levels(SIGMAS[top] * 4.0, top), (top, 1.0));
    let mut last = 0.0;
    for k in 1..400 {
        let sigma = k as f32 * 0.5;
        let (n, share) = levels(sigma, top);
        let (a, b) = (SIGMAS[n - 1], SIGMAS[n]);
        let got = (a * a + share * (b * b - a * a)).sqrt();
        assert!((got - sigma.clamp(SIGMAS[0], SIGMAS[top])).abs() < 1e-3, "sigma {sigma}: {got}");
        assert!(got >= last);
        last = got;
    }
}
