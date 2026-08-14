use std::io::Write as _;
use std::process::Stdio;

use super::*;

/// A stale copy is noticed by activation and by nothing else, so this drives
/// the only path that can notice one.
///
/// Nothing rebuilds at hook time, by design, so the published copy stays
/// exactly as current as the last activation left it and no amount of running
/// the hook will reveal that it is behind. Overwriting it here stands in for
/// the ordinary case of a copy left by an older build: to activation both are
/// simply bytes that differ from the ones it is holding, which is the entire
/// comparison it makes.
pub(super) fn assert_stale_entry_point_is_republished(scratch: &Scratch, source: &Path) {
    let published = published_entry_point(scratch.path());
    fs::write(&published, b"#!/bin/sh\nexit 0\n").expect("stale the published entry point");
    let mut reactivated = Vec::new();
    xtask::activate_hooks(scratch.path(), source, &mut reactivated)
        .expect("re-run activation over a stale entry point");
    let reactivated = String::from_utf8(reactivated).unwrap();
    assert!(
        reactivated.contains("published hook entry point"),
        "activation reports the copy it replaced: {reactivated}"
    );
    assert_eq!(
        fs::read(&published).expect("republished entry point reads"),
        fs::read(source).expect("entry point source reads"),
        "a stale copy is replaced by the bytes activation was handed"
    );
}

/// Byte-identical is not the same as usable, and Git needs the second one.
///
/// Activation's current-check compares bytes, so a copy left unexecutable by an
/// interrupted publish, a restrictive umask, or a restore from an archive reads
/// as current and activation would walk past it. Git then fails *every*
/// reference transaction in the repository, including the ones the guard exists
/// to allow. Clearing the bit on the copy the previous assertion just
/// republished isolates the mode: those bytes are already the ones activation
/// holds, so nothing but the mode is left for a passing run to be crediting.
pub(super) fn assert_unusable_entry_point_is_repaired(scratch: &Scratch, source: &Path) {
    let published = published_entry_point(scratch.path());
    fs::set_permissions(&published, fs::Permissions::from_mode(0o644))
        .expect("clear the published entry point's executable bit");
    xtask::activate_hooks(scratch.path(), source, &mut Vec::new())
        .expect("re-run activation over an unexecutable entry point");
    assert_eq!(
        mode(&published),
        0o755,
        "activation restores an entry point Git can exec"
    );

    // Creation is the one transaction the guard admits without looking at
    // history, which makes it the probe whose failure can only mean the hook
    // never ran: an unexecutable target fails the exec, the hook script exits
    // nonzero, and Git aborts an update that abandons nothing at all. The probe
    // reference is left in place afterwards on purpose -- deleting it would
    // point the guard at upstream-merged commits and be refused, which is the
    // guard doing its job rather than a cleanup worth arranging around.
    let probe = "refs/heads/entry-point-repair-probe";
    let head = rev_parse(scratch, "refs/heads/main");
    let created = Command::new("git")
        .args(["update-ref", probe, &head])
        .current_dir(scratch.path())
        .output()
        .expect("create the entry point probe reference");
    assert_success(
        &created,
        "create a reference through the repaired entry point",
    );
    assert_eq!(
        rev_parse(scratch, probe),
        head,
        "the admitted transaction was committed rather than aborted"
    );
}

/// Activation is the only writer of the guard stamp, so the audit is what makes
/// a stale publication visible to anyone who did not just run activation.
///
/// Build A is the real xtask executable activation publishes. Build B changes
/// `PROTECTED_COMMIT_LIMIT`, a runtime bound that changes the guard's refusal
/// account, and reaches the same production audit with the digest that source
/// would compile. B must name and refuse A. The explicit source-to-compiled
/// digest equality is what makes severing `include_str!` fail rather than
/// letting a synthetic sidecar mutation stand in for a build transition.
///
/// An absent stamp is refused separately. Re-activating at the end restores the
/// stamp for the assertions that follow and proves the already-current branch
/// of activation stamps as well, which is the branch a second lane running
/// activation over the first lane's copy takes.
pub(super) fn assert_guard_audit_refuses_a_prior_build(scratch: &Scratch, source: &Path) {
    let stamp = published_guard_stamp(scratch.path());
    let published = fs::read_to_string(&stamp).expect("activation stamps the guard it published");
    let published = published.trim().to_owned();
    let guard_a_source = include_str!("../../src/reference_transaction.rs");
    let behavior_a = "const PROTECTED_COMMIT_LIMIT: usize = 5;";
    let behavior_b = "const PROTECTED_COMMIT_LIMIT: usize = 6;";
    assert_eq!(
        guard_a_source.matches(behavior_a).count(),
        1,
        "the fixture changes one runtime guard bound rather than arbitrary prose"
    );
    let guard_b_source = guard_a_source.replacen(behavior_a, behavior_b, 1);
    let guard_a = xtask::compiled_reference_transaction_guard_digest();
    assert_eq!(
        guard_a,
        xtask::reference_transaction_guard_digest(guard_a_source.as_bytes()),
        "the published build identity is bound to the guard source it carries"
    );
    assert_eq!(published, format!("{guard_a:016x}"));
    let guard_b = xtask::reference_transaction_guard_digest(guard_b_source.as_bytes());
    assert_ne!(
        guard_b, guard_a,
        "changing the number of protected commits the guard reports moves the next build"
    );

    let mut current = Vec::new();
    xtask::audit_entry_point(scratch.path(), &mut current)
        .expect("a freshly published guard is this build's");
    let current = String::from_utf8(current).unwrap();
    assert!(
        current.contains(&published),
        "the audit names the guard it matched: {current}"
    );

    let stale = xtask::audit_entry_point_for_guard(scratch.path(), guard_b, &mut Vec::new())
        .expect_err("build B refuses the guard published by build A")
        .to_string();
    assert!(stale.contains(&published), "{stale}");
    assert!(stale.contains(&format!("{guard_b:016x}")), "{stale}");
    assert!(stale.contains("cargo xtask activate-hooks"), "{stale}");

    fs::remove_file(&stamp).expect("remove the published guard stamp");
    let unstamped = xtask::audit_entry_point(scratch.path(), &mut Vec::new())
        .expect_err("an entry point published before stamping existed is refused")
        .to_string();
    assert!(unstamped.contains("no stamp"), "{unstamped}");

    xtask::activate_hooks(scratch.path(), source, &mut Vec::new())
        .expect("re-run activation over an already-current entry point");
    assert_eq!(
        fs::read_to_string(&stamp)
            .expect("the already-current branch stamps too")
            .trim(),
        published
    );
    xtask::audit_entry_point(scratch.path(), &mut Vec::new())
        .expect("the restored stamp is this build's");
}

/// An absent entry point aborts ref updates rather than degrading, which is
/// what makes activation a precondition of the install rather than a polish
/// step, and is now written in `README.md` where operators read it.
///
/// The claim is worth pinning because the alternative is plausible enough to
/// have been assumed: a hook that could not find its program might exit zero
/// and let the update through, which is how most optional tooling behaves and
/// is exactly the failure the guard must not have. Restoring afterwards also
/// records that absence self-heals -- the read fails and the copy is rewritten
/// -- so only the wrong-mode case above needed code to repair it.
pub(super) fn assert_an_absent_entry_point_aborts_ref_updates(scratch: &Scratch, source: &Path) {
    let published = published_entry_point(scratch.path());
    fs::remove_file(&published).expect("remove the published entry point");
    let probe = "refs/heads/entry-point-absence-probe";
    let aborted = Command::new("git")
        .args(["update-ref", probe, &rev_parse(scratch, "refs/heads/main")])
        .current_dir(scratch.path())
        .output()
        .expect("attempt a reference update with no entry point published");
    assert_ne!(
        aborted.status.code(),
        Some(0),
        "an absent entry point aborts rather than admitting: {}",
        streams(&aborted)
    );
    assert!(
        String::from_utf8_lossy(&aborted.stderr).contains("aborted by hook"),
        "Git names the hook as the abort's cause: {}",
        streams(&aborted)
    );
    let listed = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", probe])
        .current_dir(scratch.path())
        .output()
        .expect("look for the reference the aborted update would have created");
    assert!(
        !listed.status.success(),
        "the aborted update left no reference behind: {}",
        streams(&listed)
    );

    xtask::activate_hooks(scratch.path(), source, &mut Vec::new())
        .expect("activation republishes an absent entry point");
    assert_eq!(mode(&published), 0o755);
}

fn published_guard_stamp(root: &Path) -> PathBuf {
    let repository = Repository::discover(root).expect("discover hook repository");
    repository
        .common_dir()
        .join(".spice-install/bin/xtask.guard")
}

/// The published copy is what the hook names, so the guarantee is read off that
/// file and not off the binary Cargo built.
///
/// Identity is asserted before the interface. A published file that merely
/// exists would let an interface check pass against some older publication, so
/// proving the bytes are the ones activation was handed is what makes the
/// answer below an answer from this build. Then the guard is driven through the
/// published path with a transaction it must refuse: an admitted one would exit
/// zero for a program that never looked at its input, while the refusal names
/// the guard's own sentence and so cannot be produced by anything else.
pub(super) fn assert_entry_point_answers_the_guard(
    scratch: &Scratch,
    source: &Path,
    prepared: &str,
) {
    let published = published_entry_point(scratch.path());
    assert_eq!(
        mode(&published),
        0o755,
        "published entry point is executable"
    );
    assert_eq!(
        fs::read(&published).expect("published entry point reads"),
        fs::read(source).expect("entry point source reads"),
        "published entry point carries the bytes activation was handed"
    );

    let refusal = run_prepared(&published, scratch.path(), prepared);
    assert_ne!(
        refusal.status.code(),
        Some(0),
        "published entry point refuses a protected rewind: {}",
        streams(&refusal)
    );
    assert!(
        String::from_utf8_lossy(&refusal.stderr).contains("refused to abandon upstream-merged"),
        "published entry point answers the reference-transaction interface: {}",
        streams(&refusal)
    );
}

fn run_prepared(program: &Path, root: &Path, prepared: &str) -> Output {
    let mut child = Command::new(program)
        .args(["reference-transaction", "prepared"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run published entry point");
    child
        .stdin
        .take()
        .expect("published entry point accepts the prepared transaction")
        .write_all(prepared.as_bytes())
        .expect("write the prepared transaction");
    child.wait_with_output().expect("collect published output")
}
