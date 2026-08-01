//! The proof layout's closed filesystem source bound.

/// The exact source compiled as the proof layout.
const LAYOUT: &str = include_str!("../src/layout.rs");

/// Discovery and process APIs cannot enter exact-path resolution unnoticed.
#[test]
fn layout_contains_no_search_process_or_ambient_root_operation() {
    for (operation, spelling) in [
        ("directory enumeration", "read_dir"),
        ("filesystem pattern matching", "glob"),
        ("subprocess launch", "Command"),
        ("ambient current-directory acquisition", "current_dir"),
    ] {
        assert!(
            !LAYOUT.contains(spelling),
            "proof layout gained {operation} through {spelling}"
        );
    }
}
