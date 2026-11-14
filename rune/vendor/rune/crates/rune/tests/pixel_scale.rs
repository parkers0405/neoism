use rune::{Color, Cx, El, Env, Runtime, col, el};

fn view(_: &(), _: &mut Cx<()>) -> El {
    col().w_full().h_full().child(
        el().key("row").absolute(0.123, 0.137).size(30.123, 20.137)
            .fill(Color::WHITE).hover(Color::BLACK).on_press(|_: &mut ()| {}),
    )
}

#[test]
fn pixel_scale_change_matches_instances_from_a_fresh_runtime() {
    let make = |scale| {
        let mut rt = Runtime::new((), view, (240.0, 150.0), Env::new(true, scale));
        rt.frame(0.0);
        rt
    };
    let mut rt = make(1.0);
    for scale in [2.0, 1.5, 1.0] {
        rt.set_viewport((240.0, 150.0), scale);
        rt.frame(0.0);
        let fresh = make(scale);
        assert_eq!(bytemuck::cast_slice::<_, u8>(&rt.instances), bytemuck::cast_slice::<_, u8>(&fresh.instances),
            "settled hover rows resnap at scale {scale}");
    }
}
