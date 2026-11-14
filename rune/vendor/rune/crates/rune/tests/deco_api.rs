use rune::{Color, text::Deco};

#[test]
fn decoration_literal_remains_source_compatible() {
    let d = Deco { rect: [0.0; 4], color: Color::BLACK, radius: 4.0 };
    assert_eq!(d.radius, 4.0);
}
