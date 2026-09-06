use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

#[expect(
    clippy::too_many_arguments,
    reason = "each path is an independent deterministic process rendezvous"
)]
pub(super) fn write_tool(
    rustfmt: &Path,
    executions: &Path,
    move_index: &Path,
    newer: &Path,
    move_tool: &Path,
    release: &Path,
    fail_execution: &Path,
    suffix: &str,
) {
    fs::write(
        rustfmt,
        format!(
            "#!/bin/sh\nif [ \"${{1-}}\" = --version ]; then\n  printf '%s\\n' 'rustfmt 1.9.0-stable (8bab26f4f6 2026-07-14)'\n  exit 0\nfi\nprintf 'start\\n' >> '{}'\nif [ -f '{}' ]; then\n  printf '%s\\n' 'pub fn value() -> u8 {{ 99 }}' > '{}'\n  /usr/bin/git -C '{}' add src/lib.rs\nfi\nif [ -f '{}' ]; then printf '%s\\n' '# moved during execution' >> '{}'; fi\nwhile [ ! -f '{}' ]; do /bin/sleep 0.01; done\nprintf 'end\\n' >> '{}'\nif [ -f '{}' ]; then exit 91; fi\n{}",
            executions.display(),
            move_index.display(),
            newer.join("src/lib.rs").display(),
            newer.display(),
            move_tool.display(),
            rustfmt.display(),
            release.display(),
            executions.display(),
            fail_execution.display(),
            suffix,
        ),
    )
    .expect("write instrumented rustfmt");
    fs::set_permissions(rustfmt, fs::Permissions::from_mode(0o700))
        .expect("make instrumented rustfmt executable");
}
