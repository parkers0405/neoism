use super::*;

fn centre(body: &'static str) -> u8 {
    let icon = Box::leak(Box::new(Icon { name: "t", view: 16.0, body }));
    icon_coverage(icon, 32)[16 * 32 + 16]
}

#[test]
fn opacity_scales_an_elements_coverage() {
    assert_eq!(centre(r#"<circle cx="8" cy="8" r="6" fill="currentColor"/>"#), 255);
    let half = centre(r#"<circle cx="8" cy="8" r="6" fill="currentColor" opacity=".5"/>"#);
    assert!((126..=129).contains(&half), "opacity .5 is half coverage, got {half}");
    let fill = centre(r#"<circle cx="8" cy="8" r="6" fill="currentColor" fill-opacity="0.25"/>"#);
    assert!((62..=65).contains(&fill), "fill-opacity, got {fill}");
    let both = centre(r#"<circle cx="8" cy="8" r="6" fill="currentColor" opacity=".5" fill-opacity=".5"/>"#);
    assert!((62..=65).contains(&both), "they multiply, got {both}");
}

#[test]
fn stroke_opacity_leaves_the_fill_alone() {
    let body = r#"<rect x="0" y="0" width="16" height="16" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-opacity=".3"/>"#;
    assert_eq!(centre(body), 255);
}
