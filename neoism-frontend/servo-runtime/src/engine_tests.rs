use super::*;
#[test]
fn desired_theme_waits_for_committed_current_document(
) -> Result<(), Box<dyn std::error::Error>> {
    let expected = Url::parse("https://artifact-1.neoism.invalid/document")?;
    let blank = Url::parse("about:blank")?;
    let old = Url::parse("https://artifact-2.neoism.invalid/document")?;
    assert!(!super::style_target_ready(
        true,
        Some(&blank),
        &expected,
        servo::LoadStatus::Complete
    ));
    assert!(!super::style_target_ready(
        true,
        Some(&old),
        &expected,
        servo::LoadStatus::Complete
    ));
    assert!(!super::style_target_ready(
        true,
        Some(&expected),
        &expected,
        servo::LoadStatus::Started
    ));
    assert!(!super::style_target_ready(
        true,
        Some(&expected),
        &expected,
        servo::LoadStatus::HeadParsed
    ));
    assert!(!super::style_target_ready(
        false,
        Some(&expected),
        &expected,
        servo::LoadStatus::Complete
    ));
    assert!(super::style_target_ready(
        true,
        Some(&expected),
        &expected,
        servo::LoadStatus::Complete
    ));
    Ok(())
}
#[test]
fn style_theme_and_viewport_mutation_retain_source_revision() -> Result<(), Error> {
    let original = ArtifactDocument {
        key: "test".into(),
        html: "<script>window.state=7</script>".into(),
        revision: 7,
        viewport: Viewport {
            width: 320,
            height: 320,
            scale: 1.0,
        },
        visible: true,
        theme: Theme::Dark,
        styles: crate::ArtifactStyles::default(),
    };
    let mut next = original.clone();
    next.styles.font_sans = "Selected Neoism Font".into();
    next.theme = Theme::Light;
    next.viewport.width = 640;
    assert!(super::same_source(&original, &next)?);
    assert_eq!(next.revision, original.revision);
    assert_eq!(next.html, original.html);
    next.html.push_str("<p>different source</p>");
    assert_eq!(
        super::same_source(&original, &next),
        Err(Error::RevisionConflict)
    );
    Ok(())
}
