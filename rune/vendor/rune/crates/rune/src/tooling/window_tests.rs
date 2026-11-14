#[test]
fn transparent_png_unpremultiplies_in_linear_space() {
    let mut pixels = [188, 0, 0, 128, 255, 255, 255, 0, 12, 34, 56, 255];
    super::straight_alpha(&mut pixels);
    assert_eq!(pixels, [255, 0, 0, 128, 0, 0, 0, 0, 12, 34, 56, 255]);
}
