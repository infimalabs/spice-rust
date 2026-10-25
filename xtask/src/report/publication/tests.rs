use super::domain;

#[test]
fn project_markers_do_not_inherit_a_publication_category_from_a_prefix() {
    let unrecognized = domain("unrecognized_private_marker");
    let dotted = domain("app.unpublished_identifier");

    assert_eq!(dotted, unrecognized);
    assert_ne!(dotted, domain("app"));
    assert!(!dotted.contains("unpublished_identifier"));
}
