use super::*;

#[test]
fn fills_the_markers_or_the_head_and_body() {
    let marked = "<head><!-- rune:css --></head><body><nav></nav><!-- rune:body --></body>";
    assert_eq!(fill(marked, ".a{}", "<main></main>"), "<head><style data-rune-static>.a{}</style></head><body><nav></nav><main></main></body>");
    let plain = "<html><head><title>x</title></head><body class=\"k\"></body></html>";
    assert_eq!(
        fill(plain, ".a{}", "<main></main>"),
        "<html><head><title>x</title><style data-rune-static>.a{}</style></head><body class=\"k\"><main></main></body></html>",
    );
}

#[test]
fn the_default_page_has_both_markers() {
    assert!(DEFAULT_TEMPLATE.contains(CSS_MARK) && DEFAULT_TEMPLATE.contains(BODY_MARK));
}
