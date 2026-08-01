use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use spice::install::{
    Action, Change, Generation, HookSelection, HookValue, Observation, Operation, Plan,
};
use wire::{Digest, Domain, Outcome, ReplyPosition};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const PRIOR_POLICY: &str = concat!(
    "[lane]\nvendor = \"codex\"\n\n",
    "[taste.polling]\nwords = [\"wait*\"]\n",
    "message = \"Wait for evidence.\"\n",
);

#[test]
fn preview_apply_repeat_and_unapply_preserve_the_existing_runtime() {
    let fixture = Fixture::main("round-trip");
    let before = fixture.snapshot();

    let preview = fixture.invoke(&["--json", "install"]);
    assert_eq!(preview.status.code(), Some(0));
    assert!(preview.stderr.is_empty());
    let preview = reply(&preview.stdout);
    assert_eq!(preview.action, Action::Preview);
    assert_eq!(fixture.snapshot(), before, "preview must not write");

    let applied = fixture.invoke(&["--json", "install", &format!("--apply={}", preview.digest)]);
    assert_eq!(applied.status.code(), Some(0));
    assert!(applied.stderr.is_empty());
    assert_eq!(reply(&applied.stdout).action, Action::Applied);
    fixture.assert_applied();

    let applied_state = fixture.snapshot();
    let repeated_preview = reply(&fixture.invoke(&["--json", "install"]).stdout);
    assert!(
        repeated_preview
            .operations
            .iter()
            .all(|operation| operation.change == Change::Retain)
    );
    let repeated = fixture.invoke(&[
        "--json",
        "install",
        &format!("--apply={}", repeated_preview.digest),
    ]);
    assert_eq!(repeated.status.code(), Some(0));
    assert_eq!(fixture.snapshot(), applied_state, "repeat apply is inert");

    let unapplied = fixture.invoke(&["install", "--unapply", "--json"]);
    assert_eq!(unapplied.status.code(), Some(0));
    assert!(unapplied.stderr.is_empty());
    assert_eq!(reply(&unapplied.stdout).action, Action::Unapplied);
    fixture.assert_unapplied();
}

#[test]
fn absent_policy_seeds_are_created_and_exactly_removed() {
    let fixture = Fixture::main("absent-seeds");
    let policy = fixture.root.join("spice.toml");
    let baseline = fixture.root.join("baseline.toml");
    fs::remove_file(&policy).expect("remove prior policy");
    fs::remove_file(&baseline).expect("remove prior baseline");
    let before = fixture.snapshot();

    let preview = reply(&fixture.invoke(&["--json", "install"]).stdout);
    for path in [&policy, &baseline] {
        let operation = preview
            .operations
            .iter()
            .find(|operation| operation.path == *path)
            .expect("seed operation");
        assert_eq!(operation.change, Change::Create);
        assert_eq!(operation.observed, Observation::Absent);
    }
    let applied = fixture.invoke(&["--json", "install", &format!("--apply={}", preview.digest)]);
    assert_eq!(applied.status.code(), Some(0));
    assert!(
        fs::read_to_string(&policy)
            .unwrap()
            .contains("[taste.polling]")
    );
    assert!(!fs::read(&baseline).unwrap().is_empty());

    assert_eq!(
        fixture
            .invoke(&["--json", "install", "--unapply"])
            .status
            .code(),
        Some(0),
    );
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn a_missing_record_refuses_and_an_absent_hook_selection_is_exactly_reversed() {
    let fixture = Fixture::main("absent-hook");
    fs::write(
        fixture.common.join("config"),
        b"[receive]\n\tdenyDeletes = true\n",
    )
    .expect("remove the prior hook selection");
    let before = fixture.snapshot();

    let missing = fixture.invoke(&["--json", "install", "--unapply"]);
    assert_eq!(missing.status.code(), Some(2));
    assert_eq!(refusal_reason(&missing.stderr), "install::record-missing");
    assert_eq!(fixture.snapshot(), before);

    let preview = reply(&fixture.invoke(&["--json", "install"]).stdout);
    assert_eq!(preview.hook.previous, HookValue::Absent);
    let applied = fixture.invoke(&["--json", "install", &format!("--apply={}", preview.digest)]);
    assert_eq!(applied.status.code(), Some(0));
    let unapplied = fixture.invoke(&["--json", "install", "--unapply"]);
    assert_eq!(unapplied.status.code(), Some(0));
    assert_eq!(reply(&unapplied.stdout).hook.selected, HookValue::Absent);
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn changed_digest_and_changed_installed_bytes_refuse_without_partial_reversal() {
    let fixture = Fixture::main("refusals");
    let before = fixture.snapshot();
    let mismatched = fixture.invoke(&["--json", "install", &format!("--apply={}", "0".repeat(64))]);
    assert_eq!(mismatched.status.code(), Some(2));
    assert!(mismatched.stdout.is_empty());
    assert_eq!(refusal_reason(&mismatched.stderr), "install::plan-changed");
    assert_eq!(fixture.snapshot(), before);

    let preview = reply(&fixture.invoke(&["--json", "install"]).stdout);
    let applied = fixture.invoke(&["--json", "install", &format!("--apply={}", preview.digest)]);
    assert_eq!(applied.status.code(), Some(0));
    let skill = fixture.root.join(".agents/skills/spice/SKILL.md");
    let installed = fs::read(&skill).expect("read installed skill");
    fs::write(&skill, b"operator edit\n").expect("change installed skill");
    let before_refusal = fixture.snapshot();

    let drift = fixture.invoke(&["--json", "install", "--unapply"]);
    assert_eq!(drift.status.code(), Some(2));
    assert_eq!(refusal_reason(&drift.stderr), "install::drift");
    assert_eq!(fixture.snapshot(), before_refusal);

    fs::write(&skill, installed).expect("restore installed skill");
    assert_eq!(
        fixture
            .invoke(&["--json", "install", "--unapply"])
            .status
            .code(),
        Some(0)
    );
}

#[cfg(unix)]
#[test]
fn accepted_digest_binds_observed_modes_and_git_config_bytes() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::main("observed-state");
    let policy = fixture.root.join("spice.toml");
    let preview = reply(&fixture.invoke(&["--json", "install"]).stdout);
    let original_mode = fs::metadata(&policy).unwrap().permissions().mode() & 0o777;
    let changed_mode = if original_mode == 0o600 { 0o644 } else { 0o600 };
    fs::set_permissions(&policy, fs::Permissions::from_mode(changed_mode))
        .expect("change observed mode");
    let changed = fixture.snapshot();

    let stale = fixture.invoke(&["--json", "install", &format!("--apply={}", preview.digest)]);
    assert_eq!(stale.status.code(), Some(2));
    assert_eq!(refusal_reason(&stale.stderr), "install::plan-changed");
    assert_eq!(fixture.snapshot(), changed);

    fs::set_permissions(&policy, fs::Permissions::from_mode(original_mode))
        .expect("restore observed mode");
    let preview = reply(&fixture.invoke(&["--json", "install"]).stdout);
    let config = fixture.common.join("config");
    let mut edited = fs::read(&config).expect("read Git config");
    edited.extend_from_slice(b"[spice-test]\n\tvalue = true\n");
    fs::write(&config, edited).expect("change unrelated Git config bytes");
    let changed = fixture.snapshot();

    let stale = fixture.invoke(&["--json", "install", &format!("--apply={}", preview.digest)]);
    assert_eq!(stale.status.code(), Some(2));
    assert_eq!(refusal_reason(&stale.stderr), "install::plan-changed");
    assert_eq!(fixture.snapshot(), changed);
}

#[cfg(unix)]
#[test]
fn stable_parent_symlinks_are_bound_and_retargeted_parents_refuse() {
    use std::os::unix::fs::symlink;

    let stable = Fixture::main("stable-parent-symlink");
    let stable_target = stable.home.join("local-target/bin");
    fs::create_dir_all(&stable_target).expect("create stable symlink target");
    symlink("local-target", stable.home.join(".local")).expect("link stable install parent");
    let preview = reply(&stable.invoke(&["--json", "install"]).stdout);
    assert_eq!(preview.operations[0].path, stable_target.join("spice-rust"));
    assert_eq!(
        stable
            .invoke(&["--json", "install", &format!("--apply={}", preview.digest)])
            .status
            .code(),
        Some(0),
    );
    let installed = stable_target.join("spice-rust").display().to_string();
    let hook = fs::read_to_string(stable.common.join(".spice/hooks/pre-commit"))
        .expect("read installed hook");
    let systemd = only_file_text(&stable.home.join(".config/systemd/user"));
    let launchd = only_file_text(&stable.home.join("Library/LaunchAgents"));
    for generated in [hook, systemd, launchd] {
        assert!(generated.contains(&installed));
    }
    assert_eq!(
        stable
            .invoke(&["--json", "install", "--unapply"])
            .status
            .code(),
        Some(0),
    );
    stable.assert_unapplied();

    let swapped = Fixture::main("swapped-parent-symlink");
    fs::create_dir_all(swapped.home.join("targets/first/bin")).expect("create first target");
    fs::create_dir_all(swapped.home.join("targets/second/bin")).expect("create second target");
    symlink("targets/first", swapped.home.join(".local")).expect("link first target");
    let preview = reply(&swapped.invoke(&["--json", "install"]).stdout);
    fs::remove_file(swapped.home.join(".local")).expect("remove first link");
    symlink("targets/second", swapped.home.join(".local")).expect("link second target");
    let before = swapped.snapshot();
    let refused = swapped.invoke(&["--json", "install", &format!("--apply={}", preview.digest)]);
    assert_eq!(refused.status.code(), Some(2));
    assert_eq!(refusal_reason(&refused.stderr), "install::plan-changed");
    assert_eq!(swapped.snapshot(), before);
    assert!(!swapped.home.join("targets/second/bin/spice-rust").exists());

    let escaped = Fixture::main("escaped-parent-symlink");
    let outside = escaped.scratch.path().join("outside-home/bin");
    fs::create_dir_all(&outside).expect("create escaped target");
    symlink(
        outside.parent().expect("outside target has a parent"),
        escaped.home.join(".local"),
    )
    .expect("link escaped install parent");
    let before = escaped.snapshot();
    let refused = escaped.invoke(&["--json", "install"]);
    assert_eq!(refused.status.code(), Some(2));
    assert_eq!(refusal_reason(&refused.stderr), "install::destination-root");
    assert_eq!(escaped.snapshot(), before);
}

#[cfg(unix)]
#[test]
fn new_replaced_parents_and_same_byte_file_generations_invalidate_the_plan() {
    let created = Fixture::main("created-parent");
    let preview = reply(&created.invoke(&["--json", "install"]).stdout);
    fs::create_dir_all(created.home.join(".local/bin")).expect("create previewed parent");
    assert_stale_plan_is_inert(&created, &preview);

    let replaced = Fixture::main("replaced-parent");
    fs::create_dir_all(replaced.home.join(".local/bin")).expect("create original parent");
    let preview = reply(&replaced.invoke(&["--json", "install"]).stdout);
    fs::rename(
        replaced.home.join(".local"),
        replaced.home.join("retired-local"),
    )
    .expect("retire original parent");
    fs::create_dir_all(replaced.home.join(".local/bin")).expect("replace parent generation");
    assert_stale_plan_is_inert(&replaced, &preview);

    let replaced_file = Fixture::main("replaced-file-generation");
    let preview = reply(&replaced_file.invoke(&["--json", "install"]).stdout);
    let policy = replaced_file.root.join("spice.toml");
    let replacement = replaced_file.root.join("replacement-policy");
    fs::write(&replacement, fs::read(&policy).expect("read policy")).expect("stage same bytes");
    fs::rename(&replacement, &policy).expect("replace policy generation");
    assert_stale_plan_is_inert(&replaced_file, &preview);
}

#[test]
fn copied_corrupt_and_noncanonical_records_refuse_without_mutation() {
    let source = applied_fixture("record-source");
    let target = applied_fixture("record-target");
    fs::copy(source.record(), target.record()).expect("copy foreign record");
    assert_record_failure_is_inert(&target, "install::record-shape");

    let noncanonical = applied_fixture("record-noncanonical");
    let mut bytes = fs::read(noncanonical.record()).expect("read canonical record");
    bytes.push(b'\n');
    fs::write(noncanonical.record(), bytes).expect("write noncanonical record");
    assert_record_failure_is_inert(&noncanonical, "install::record-shape");

    let corrupt = applied_fixture("record-corrupt");
    fs::write(corrupt.record(), b"{not-json").expect("write corrupt record");
    assert_record_failure_is_inert(&corrupt, "install::record-invalid");

    let old_version = applied_fixture("record-old-version");
    let mut record = read_record(&old_version);
    record.version -= 1;
    write_record(&old_version, &record);
    assert_record_failure_is_inert(&old_version, "install::record-version");
}

#[test]
fn resigned_out_of_root_reordered_and_prior_tampered_records_refuse() {
    let out_of_root = applied_fixture("record-out-of-root");
    let mut record = read_record(&out_of_root);
    let original = record.operations[0].path.clone();
    let outside = out_of_root.scratch.path().join("outside-target");
    record.operations[0].path = outside.clone();
    record
        .writes
        .iter_mut()
        .find(|write| write.path == original)
        .expect("first write")
        .path = outside;
    resign(&mut record);
    write_record(&out_of_root, &record);
    assert_record_failure_is_inert(&out_of_root, "install::record-shape");

    let reordered = applied_fixture("record-reordered");
    let mut record = read_record(&reordered);
    record.operations.swap(0, 1);
    record.writes.swap(0, 1);
    resign(&mut record);
    write_record(&reordered, &record);
    assert_record_failure_is_inert(&reordered, "install::record-shape");

    let duplicated = applied_fixture("record-duplicated");
    let mut record = read_record(&duplicated);
    record.operations[1] = record.operations[0].clone();
    record.writes[1] = record.writes[0].clone();
    resign(&mut record);
    write_record(&duplicated, &record);
    assert_record_failure_is_inert(&duplicated, "install::record-shape");

    let tampered = applied_fixture("record-prior-tampered");
    let mut record = read_record(&tampered);
    record.writes.swap(0, 4);
    resign(&mut record);
    write_record(&tampered, &record);
    assert_record_failure_is_inert(&tampered, "install::record-shape");

    let mismatched = applied_fixture("record-plan-mismatch");
    let mut record = read_record(&mismatched);
    record.plan = Digest::compute(Domain::InstallPlan, b"mismatched plan");
    write_record(&mismatched, &record);
    assert_record_failure_is_inert(&mismatched, "install::record-shape");
}

#[test]
fn linked_worktree_uses_the_shared_git_state_and_text_matches_json() {
    let fixture = Fixture::linked("linked");
    let text = fixture.invoke(&["install"]);
    let json = fixture.invoke(&["--json", "install"]);
    assert_eq!(text.status.code(), Some(0));
    assert_eq!(json.status.code(), Some(0));
    let text_plan: Plan = wire::decode(&text.stdout).expect("text body is the plan");
    let json_plan = reply(&json.stdout);
    assert_eq!(text_plan, json_plan);
    assert_eq!(
        text_plan.hook.config,
        fixture.common.join("config"),
        "hook selection belongs to shared Git state",
    );
    assert!(
        text_plan
            .operations
            .iter()
            .filter(|operation| operation.path.to_string_lossy().contains("/.spice/hooks/"))
            .all(|operation| operation.path.starts_with(fixture.common.join(".spice")))
    );
}

#[cfg(unix)]
#[test]
fn an_interrupted_late_publication_restores_every_earlier_destination() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::main("interruption");
    let launch_agents = fixture.home.join("Library/LaunchAgents");
    fs::create_dir_all(&launch_agents).expect("create late destination");
    let preview = reply(&fixture.invoke(&["--json", "install"]).stdout);
    fs::set_permissions(&launch_agents, fs::Permissions::from_mode(0o555))
        .expect("make late destination unwritable");
    let before = fixture.snapshot();

    let interrupted =
        fixture.invoke(&["--json", "install", &format!("--apply={}", preview.digest)]);
    fs::set_permissions(&launch_agents, fs::Permissions::from_mode(0o755))
        .expect("restore destination permission");

    assert_eq!(interrupted.status.code(), Some(1));
    assert!(interrupted.stdout.is_empty());
    assert_eq!(fixture.snapshot_without_modes(), before.without_modes());
    assert!(!fixture.record().exists());
}

#[cfg(unix)]
#[test]
fn an_interrupted_unapply_restores_the_complete_installed_state() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::main("unapply-interruption");
    let preview = reply(&fixture.invoke(&["--json", "install"]).stdout);
    let applied = fixture.invoke(&["--json", "install", &format!("--apply={}", preview.digest)]);
    assert_eq!(applied.status.code(), Some(0));
    let skill_directory = fixture.root.join(".agents/skills/spice");
    fs::set_permissions(&skill_directory, fs::Permissions::from_mode(0o555))
        .expect("make prior destination unwritable");
    let installed = fixture.snapshot();

    let interrupted = fixture.invoke(&["--json", "install", "--unapply"]);
    fs::set_permissions(&skill_directory, fs::Permissions::from_mode(0o755))
        .expect("restore prior destination permission");

    assert_eq!(interrupted.status.code(), Some(1));
    assert_eq!(fixture.snapshot_without_modes(), installed.without_modes());
    assert!(fixture.record().is_file());
    assert_eq!(
        fixture
            .invoke(&["--json", "install", "--unapply"])
            .status
            .code(),
        Some(0),
    );
}

fn reply(bytes: &[u8]) -> Plan {
    let outcome: Outcome<Plan> = wire::decode(bytes).expect("typed install outcome");
    let Outcome::Reply(reply) = outcome else {
        panic!("install must reply: {outcome:?}");
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    reply.body
}

fn refusal_reason(bytes: &[u8]) -> String {
    let outcome: Outcome<Plan> = wire::decode(bytes).expect("typed install refusal");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("install must refuse: {outcome:?}");
    };
    refusal.reason
}

fn failure_reason(bytes: &[u8]) -> String {
    let outcome: Outcome<Plan> = wire::decode(bytes).expect("typed install failure");
    match outcome {
        Outcome::Refusal(refusal) => refusal.reason,
        Outcome::Fault(fault) => fault.reason,
        other => panic!("install must fail: {other:?}"),
    }
}

fn assert_stale_plan_is_inert(fixture: &Fixture, preview: &Plan) {
    let before = fixture.snapshot();
    let stale = fixture.invoke(&["--json", "install", &format!("--apply={}", preview.digest)]);
    assert_eq!(stale.status.code(), Some(2));
    assert_eq!(refusal_reason(&stale.stderr), "install::plan-changed");
    assert_eq!(fixture.snapshot(), before);
    assert!(!fixture.home.join(".local/bin/spice-rust").exists());
}

fn applied_fixture(name: &str) -> Fixture {
    let fixture = Fixture::main(name);
    let preview = reply(&fixture.invoke(&["--json", "install"]).stdout);
    let applied = fixture.invoke(&["--json", "install", &format!("--apply={}", preview.digest)]);
    assert_eq!(applied.status.code(), Some(0));
    fixture
}

fn assert_record_failure_is_inert(fixture: &Fixture, expected: &str) {
    let before = fixture.snapshot();
    let failed = fixture.invoke(&["--json", "install", "--unapply"]);
    assert_ne!(failed.status.code(), Some(0));
    assert_eq!(failure_reason(&failed.stderr), expected);
    assert_eq!(fixture.snapshot(), before);
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredRecord {
    version: u16,
    repository: PathBuf,
    home: PathBuf,
    plan: Digest,
    operations: Vec<Operation>,
    selection: HookSelection,
    hook: StoredHook,
    writes: Vec<StoredWrite>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredHook {
    config: PathBuf,
    previous: Option<String>,
    selected: String,
    prior: StoredSnapshot,
    installed: Digest,
    mode: u32,
    changed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredWrite {
    path: PathBuf,
    prior: StoredSnapshot,
    installed: Digest,
    mode: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum StoredSnapshot {
    Absent,
    File {
        bytes: Vec<u8>,
        mode: u32,
        generation: Generation,
    },
}

#[derive(Serialize)]
struct StoredPlanIdentity<'a> {
    version: u16,
    repository: &'a Path,
    operations: &'a [Operation],
    hook: &'a HookSelection,
}

fn read_record(fixture: &Fixture) -> StoredRecord {
    let bytes = fs::read(fixture.record()).expect("read install record");
    wire::decode(&bytes).expect("decode install record")
}

fn write_record(fixture: &Fixture, record: &StoredRecord) {
    let bytes = wire::encode(record).expect("encode install record");
    fs::write(fixture.record(), bytes).expect("write install record");
}

fn resign(record: &mut StoredRecord) {
    let identity = StoredPlanIdentity {
        version: record.version,
        repository: &record.repository,
        operations: &record.operations,
        hook: &record.selection,
    };
    let bytes = wire::encode(&identity).expect("encode recorded plan identity");
    record.plan = Digest::compute(Domain::InstallPlan, &bytes);
}

struct Fixture {
    scratch: Scratch,
    root: PathBuf,
    common: PathBuf,
    home: PathBuf,
    prior_policy: Vec<u8>,
    prior_baseline: Vec<u8>,
    prior_skill: Vec<u8>,
    prior_entrypoint: Vec<u8>,
}

impl Fixture {
    fn main(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let root = scratch.path().join("repository");
        let common = root.join(".git");
        fs::create_dir_all(&common).expect("create Git directory");
        Self::finish(scratch, root, common)
    }

    fn linked(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let origin = scratch.path().join("origin");
        let common = origin.join(".git");
        let git = common.join("worktrees/lane");
        let root = scratch.path().join("linked repository");
        fs::create_dir_all(&git).expect("create linked Git directory");
        fs::create_dir_all(&root).expect("create linked worktree");
        fs::write(root.join(".git"), format!("gitdir: {}\n", git.display()))
            .expect("write Git pointer");
        fs::write(git.join("commondir"), b"../..\n").expect("write common pointer");
        Self::finish(scratch, root, common)
    }

    fn finish(scratch: Scratch, root: PathBuf, common: PathBuf) -> Self {
        let root = fs::canonicalize(root).expect("canonical worktree");
        let common = fs::canonicalize(common).expect("canonical common Git directory");
        let home = scratch.path().join("home");
        fs::create_dir_all(home.join("bin")).expect("create fixture home");
        let home = fs::canonicalize(home).expect("canonical fixture home");
        fs::create_dir_all(root.join(".agents/skills/spice")).expect("create skill parent");
        let prior_policy = PRIOR_POLICY.as_bytes().to_vec();
        let prior_baseline = b"version = 9\n".to_vec();
        let prior_skill = b"python harness skill\n".to_vec();
        let prior_entrypoint = b"python harness entrypoint\n".to_vec();
        fs::write(root.join("spice.toml"), &prior_policy).expect("write policy");
        fs::write(root.join("baseline.toml"), &prior_baseline).expect("write baseline");
        fs::write(root.join(".agents/skills/spice/SKILL.md"), &prior_skill)
            .expect("write prior skill");
        fs::write(home.join("bin/spice"), &prior_entrypoint).expect("write Python entrypoint");
        fs::write(
            common.join("config"),
            b"[core]\n\thooksPath = \".python/hooks\"\n[receive]\n\tdenyDeletes = true\n",
        )
        .expect("write Git config");
        Self {
            scratch,
            root,
            common,
            home,
            prior_policy,
            prior_baseline,
            prior_skill,
            prior_entrypoint,
        }
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .output()
            .expect("invoke spice")
    }

    fn record(&self) -> PathBuf {
        self.common.join(".spice/install")
    }

    fn assert_applied(&self) {
        assert!(self.record().is_file());
        let config = fs::read_to_string(self.common.join("config")).expect("read Git config");
        assert!(config.contains(&self.common.join(".spice/hooks").display().to_string()));
        assert!(!config.contains(".python/hooks"));
        let policy = fs::read_to_string(self.root.join("spice.toml")).expect("read policy");
        assert!(policy.contains("vendor = \"codex\""));
        for name in [
            "polling",
            "fallbacks",
            "compatibility",
            "shims",
            "aliases",
            "legacy",
        ] {
            assert!(
                policy.contains(&format!("[taste.{name}]")),
                "missing {name}"
            );
        }
        assert_eq!(
            fs::read(self.home.join("bin/spice")).expect("read entrypoint"),
            self.prior_entrypoint,
        );
        let installed = self.home.join(".local/bin/spice-rust");
        assert_eq!(
            fs::read(&installed).expect("read side-by-side Rust executable"),
            fs::read(env!("CARGO_BIN_EXE_spice")).expect("read invoked Rust executable"),
        );
        let hook = fs::read_to_string(self.common.join(".spice/hooks/pre-commit"))
            .expect("read installed hook");
        let systemd = only_file_text(&self.home.join(".config/systemd/user"));
        let launchd = only_file_text(&self.home.join("Library/LaunchAgents"));
        for generated in [&hook, &systemd, &launchd] {
            assert!(generated.contains(&installed.display().to_string()));
            assert!(!generated.contains(&self.home.join("bin/spice").display().to_string()));
            assert!(!generated.contains(env!("CARGO_BIN_EXE_spice")));
        }
    }

    fn assert_unapplied(&self) {
        assert!(!self.record().exists());
        assert_eq!(
            fs::read(self.root.join("spice.toml")).unwrap(),
            self.prior_policy
        );
        assert_eq!(
            fs::read(self.root.join("baseline.toml")).unwrap(),
            self.prior_baseline,
        );
        assert_eq!(
            fs::read(self.root.join(".agents/skills/spice/SKILL.md")).unwrap(),
            self.prior_skill,
        );
        assert_eq!(
            fs::read(self.home.join("bin/spice")).unwrap(),
            self.prior_entrypoint,
        );
        assert!(!self.home.join(".local/bin/spice-rust").exists());
        let config = fs::read_to_string(self.common.join("config")).expect("read Git config");
        assert!(config.contains(".python/hooks"));
        assert!(!config.contains(&self.common.join(".spice/hooks").display().to_string()));
    }

    fn snapshot(&self) -> SnapshotTree {
        SnapshotTree::read(self.scratch.path())
    }

    fn snapshot_without_modes(&self) -> SnapshotTree {
        self.snapshot().without_modes()
    }
}

fn only_file_text(directory: &Path) -> String {
    let mut files = directory
        .read_dir()
        .expect("read generated template directory")
        .map(|entry| entry.expect("generated template entry").path())
        .collect::<Vec<_>>();
    files.sort();
    assert_eq!(files.len(), 1, "one generated template");
    fs::read_to_string(&files[0]).expect("read generated template")
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SnapshotTree(Vec<(PathBuf, u32, Vec<u8>)>);

impl SnapshotTree {
    fn read(root: &Path) -> Self {
        let mut entries = Vec::new();
        walk(root, root, &mut entries);
        Self(entries)
    }

    fn without_modes(mut self) -> Self {
        for (_, mode, _) in &mut self.0 {
            *mode = 0;
        }
        self
    }
}

fn walk(root: &Path, path: &Path, entries: &mut Vec<(PathBuf, u32, Vec<u8>)>) {
    let mut children = fs::read_dir(path)
        .expect("read snapshot directory")
        .map(|entry| entry.expect("snapshot entry").path())
        .collect::<Vec<_>>();
    children.sort();
    for child in children {
        let metadata = fs::symlink_metadata(&child).expect("snapshot metadata");
        if metadata.is_dir() {
            walk(root, &child, entries);
        } else if metadata.is_file() {
            entries.push((
                child
                    .strip_prefix(root)
                    .expect("relative snapshot")
                    .to_path_buf(),
                mode(&metadata),
                fs::read(&child).expect("snapshot file"),
            ));
        }
    }
}

#[cfg(unix)]
fn mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode(_metadata: &fs::Metadata) -> u32 {
    0
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "spice-install-{}-{name}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create scratch root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove scratch root");
    }
}
