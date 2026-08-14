use super::*;

/// The installed reference hook belongs to Spice, not xtask activation.
pub(super) fn assert_reference_transaction_hook_targets_installed_spice(scratch: &Scratch) {
    let hook = common_hooks(scratch).join("reference-transaction");
    let bytes = fs::read(&hook).expect("read installed reference-transaction hook");
    let text = std::str::from_utf8(&bytes).expect("installed hook is UTF-8");

    assert_eq!(text.lines().count(), 2);
    assert_eq!(text.matches("exec ").count(), 1);
    assert!(
        text.lines().nth(1).is_some_and(|line| {
            line.starts_with("exec '")
                && line.ends_with("/.spice-install/bin/spice' gate reference-transaction \"$@\"")
        }),
        "installed hook must directly enter the Spice gate: {text}"
    );
    for forbidden in ["cargo", "xtask", "git rev-parse", "$("] {
        assert!(
            !text.contains(forbidden),
            "installed reference hook contains {forbidden:?}: {text}"
        );
    }
}
