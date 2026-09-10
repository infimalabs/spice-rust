use std::path::Path;

use super::*;

#[test]
fn paths_sort_and_every_operation_kind_is_summarized() {
    let arguments = r#"{"changes":[{"path":"/workspace/z.rs","kind":"update"},{"path":"/workspace/a.rs","kind":"add"},{"path":"/workspace/m.rs","kind":"update"}]}"#;

    assert_eq!(
        activity_in(
            lane(1),
            &tool(VENDOR_ID, call("file_change", arguments)),
            &mut CallPreviews::default(),
            Some(Path::new("/workspace")),
        ),
        "file_change: a.rs +2 paths [add,update]"
    );
}

#[test]
fn paths_stay_bounded_and_fail_closed_without_relative_authority() {
    let long = "segment/".repeat(MAX_CHARS);
    let arguments = format!(
        r#"{{"changes":[{{"path":"/workspace/{long}first.rs","kind":"update"}},{{"path":"/workspace/second.rs","kind":"delete"}}]}}"#
    );
    let mut previews = CallPreviews::default();
    let preview = activity_in(
        lane(1),
        &tool(VENDOR_ID, call("file_change", &arguments)),
        &mut previews,
        Some(Path::new("/workspace")),
    );
    let result = activity_in(
        lane(1),
        &tool(VENDOR_ID, succeeded()),
        &mut previews,
        Some(Path::new("/workspace")),
    );
    assert!(preview.chars().count() <= MAX_CHARS);
    assert!(preview.contains(" +1 paths [delete,update]"));
    assert!(result.chars().count() <= MAX_CHARS);
    assert!(result.ends_with(" succeeded"));
    assert!(!preview.contains("/workspace/"));

    for unsafe_arguments in [
        r#"{"changes":[{"path":"/outside/secret.rs","kind":"update"}]}"#,
        r#"{"changes":[{"kind":"update"}]}"#,
    ] {
        assert_eq!(
            activity_in(
                lane(1),
                &tool(VENDOR_ID, call("file_change", unsafe_arguments)),
                &mut CallPreviews::default(),
                Some(Path::new("/workspace")),
            ),
            "file_change: changes[1]"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_non_utf8_worktree_never_leaks_an_absolute_path() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    let worktree = PathBuf::from(OsString::from_vec(b"/workspace/\xff".to_vec()));
    let preview = activity_in(
        lane(1),
        &tool(
            VENDOR_ID,
            call(
                "file_change",
                r#"{"changes":[{"path":"/workspace/secret.rs","kind":"update"}]}"#,
            ),
        ),
        &mut CallPreviews::default(),
        Some(&worktree),
    );

    assert_eq!(preview, "file_change: changes[1]");
    assert!(!preview.contains("/workspace/"));
}
