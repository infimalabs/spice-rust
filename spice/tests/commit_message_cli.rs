#![cfg(unix)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use spice::cli::{Guard, JournalAccess, Landing, Principal, ResultClass};
use spice::install::{apply, installed_git_hooks, plan};
use spice::repository::Repository;
use wire::Outcome;

#[path = "support/git_repository.rs"]
mod git_repository;

const FOLD_NOTICE: &str = concat!(
    "spice commit-msg: auto-folded commit message body lines; ",
    "keep body prose wrapped at 100 chars\n"
);
const VALID_ORIGIN: &str =
    "Spice-Origin: v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=HOOKS-1kJQnmG8";

#[test]
fn typed_contract_and_help_publish_the_exact_offline_leaf() {
    let contract = spice::cli::contracts()
        .iter()
        .find(|contract| contract.path == ["gate", "commit-msg"])
        .expect("commit-message contract is registered");
    assert_eq!(contract.syntax, "gate commit-msg MESSAGE_FILE");
    assert_eq!(contract.principal, Principal::Local);
    assert_eq!(contract.journal, JournalAccess::None);
    assert_eq!(contract.guards, &[Guard::Daemonless, Guard::Offline]);
    assert!(contract.tools.is_empty());
    assert_eq!(contract.result, ResultClass::Reply);
    assert_eq!(
        spice::cli::landing(contract.path),
        Landing::Landed,
        "the typed leaf has executable dispatch"
    );

    let help = bare(&["gate", "commit-msg", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(help.stderr.is_empty());
    let help = String::from_utf8(help.stdout).expect("help is UTF-8");
    assert!(help.contains("Usage: spice gate commit-msg MESSAGE_FILE"));
}

#[test]
fn parser_rejects_missing_and_extra_message_files_exactly() {
    for arguments in [
        vec!["gate", "commit-msg"],
        vec!["gate", "commit-msg", "one", "two"],
    ] {
        let output = bare(&arguments);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let diagnostic = String::from_utf8(output.stderr).expect("usage is UTF-8");
        assert!(diagnostic.contains("Usage: spice gate commit-msg MESSAGE_FILE"));
    }
}

#[test]
fn unchanged_and_rewritten_messages_preserve_the_hook_stream_contract() {
    let scratch = Scratch::new("rewrite");
    let unchanged = scratch.path().join("unchanged message");
    let original = format!("Preserve current policy\n\n{VALID_ORIGIN}\n");
    fs::write(&unchanged, &original).expect("write unchanged message");

    let output = invoke(&scratch, &["gate", "commit-msg", path(&unchanged)]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(fs::read_to_string(&unchanged).unwrap(), original);

    let rewritten = scratch.path().join("message with spaces and 'apostrophe'");
    fs::write(
        &rewritten,
        format!(
            "Preserve current folding\n\n{}\n\n{VALID_ORIGIN}\n",
            "body prose ".repeat(16)
        ),
    )
    .expect("write long message");
    let output = invoke(&scratch, &["gate", "commit-msg", path(&rewritten)]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), FOLD_NOTICE);
    assert!(
        fs::read_to_string(&rewritten)
            .unwrap()
            .lines()
            .all(|line| line.chars().count() <= spice::commit_message::WRAP_LIMIT)
    );

    let machine = invoke(
        &scratch,
        &["--json", "gate", "commit-msg", path(&rewritten)],
    );
    assert_eq!(machine.status.code(), Some(0));
    assert!(machine.stdout.is_empty());
    assert!(machine.stderr.is_empty());
}

#[test]
fn hygiene_origin_and_current_trailer_policy_are_refusals() {
    let scratch = Scratch::new("refusal");
    let message = scratch.path().join("COMMIT_EDITMSG");

    fs::write(&message, "WIP\n\nliteral \\n evidence\n").expect("write hygiene refusal");
    let hygiene = invoke(&scratch, &["gate", "commit-msg", path(&message)]);
    assert_eq!(hygiene.status.code(), Some(2));
    assert!(hygiene.stdout.is_empty());
    let diagnostic = String::from_utf8(hygiene.stderr).unwrap();
    assert!(diagnostic.contains("gate::commit-message-refused"));
    assert!(diagnostic.contains("literal '\\n'"));
    assert!(diagnostic.contains("is a placeholder"));

    fs::write(
        &message,
        "Preserve native provenance\n\nSpice-Origin: v2 unresolved\n",
    )
    .expect("write origin refusal");
    let origin = invoke(&scratch, &["gate", "commit-msg", path(&message)]);
    assert_eq!(origin.status.code(), Some(2));
    assert!(origin.stdout.is_empty());
    let diagnostic = String::from_utf8(origin.stderr).unwrap();
    assert!(diagnostic.contains("malformed Spice-Origin trailer"));
    assert!(diagnostic.contains("Spice-Origin: v2 unresolved"));

    fs::write(
        scratch.path().join("spice-rust.toml"),
        "[policy.commit_message]\nblocked_trailers = [\"Co-Authored-By\"]\n",
    )
    .expect("write current trailer policy");
    fs::write(
        &message,
        "Preserve native attribution\n\nCo-Authored-By: Agent <agent@example.test>\n",
    )
    .expect("write blocked trailer");
    let blocked = invoke(&scratch, &["--json", "gate", "commit-msg", path(&message)]);
    assert_eq!(blocked.status.code(), Some(2));
    assert!(blocked.stdout.is_empty());
    let outcome: Outcome<()> = wire::decode(&blocked.stderr).expect("refusal is a typed outcome");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("blocked trailer must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "gate::commit-message-refused");
    let wire::Refusal::Evidence { evidence, .. } = refusal else {
        panic!("commit-message refusal must retain immediate evidence");
    };
    assert!(evidence.contains("uses blocked trailer co-authored-by"));
}

#[test]
fn invalid_message_bytes_are_an_environment_fault() {
    let scratch = Scratch::new("invalid-utf8");
    let message = scratch.path().join("COMMIT_EDITMSG");
    fs::write(&message, [b'P', 0xff, b'\n']).expect("write invalid UTF-8 message");

    let output = invoke(&scratch, &["--json", "gate", "commit-msg", path(&message)]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let outcome: Outcome<()> = wire::decode(&output.stderr).expect("fault is a typed outcome");
    let Outcome::Fault(fault) = outcome else {
        panic!("invalid bytes must fault: {outcome:?}");
    };
    assert_eq!(fault.reason, "gate::commit-message-unavailable");
    assert!(fault.detail.contains("could not read"));
    assert!(fault.detail.contains("stream did not contain valid UTF-8"));
}

#[test]
fn unix_message_file_paths_need_not_be_utf8() {
    let scratch = Scratch::new("non-utf8-path");
    let name = OsString::from_vec(b"message-\xff".to_vec());
    let message = scratch.path().join(name);
    let created = match fs::write(&message, "Preserve native path bytes\n") {
        Ok(()) => true,
        Err(error) if error.raw_os_error() == Some(92) => false,
        Err(error) => panic!("write native-path message: {error}"),
    };

    let output = invoke_os(
        &scratch,
        &[
            OsStr::new("--json"),
            OsStr::new("gate"),
            OsStr::new("commit-msg"),
            message.as_os_str(),
        ],
    );
    assert!(output.stdout.is_empty());
    if created {
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
    } else {
        assert_eq!(output.status.code(), Some(1));
        let outcome: Outcome<()> =
            wire::decode(&output.stderr).expect("native path reaches the typed command rim");
        let Outcome::Fault(fault) = outcome else {
            panic!("missing native path must fault after parsing: {outcome:?}");
        };
        assert_eq!(fault.reason, "gate::commit-message-unavailable");
    }
}

#[test]
fn installed_hook_executes_the_current_spice_leaf_without_a_build_system() {
    let scratch = Scratch::new("installed-hook");
    let repository = Repository::discover(scratch.path()).expect("discover fixture repository");
    let source = Path::new(env!("CARGO_BIN_EXE_spice"));
    let preview = plan(&repository, Some(source)).expect("plan installed Spice");
    apply(
        &repository,
        Some(source),
        &preview.digest().expect("digest install preview"),
    )
    .expect("install current Spice");
    let hook = installed_git_hooks(&repository).join("commit-msg");
    let launcher = fs::read_to_string(&hook).expect("read installed hook");
    assert!(!launcher.contains("cargo"));
    assert!(!launcher.contains("xtask"));
    assert!(launcher.contains(" gate commit-msg \"$@\""));

    let message = scratch.path().join("hook message");
    fs::write(
        &message,
        format!("Preserve installed hook\n\n{}\n", "hook prose ".repeat(16)),
    )
    .expect("write hook message");
    let output = Command::new(&hook)
        .arg(&message)
        .current_dir(scratch.path())
        .output()
        .expect("installed hook launches");
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), FOLD_NOTICE);
}

fn bare(arguments: &[&str]) -> Output {
    command().args(arguments).output().expect("Spice launches")
}

fn invoke(scratch: &Scratch, arguments: &[&str]) -> Output {
    command()
        .args(arguments)
        .current_dir(scratch.path())
        .output()
        .expect("Spice launches")
}

fn invoke_os(scratch: &Scratch, arguments: &[&OsStr]) -> Output {
    command()
        .args(arguments)
        .current_dir(scratch.path())
        .output()
        .expect("Spice launches")
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
    command
        .env_remove("CLICOLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("FORCE_COLOR")
        .env("NO_COLOR", "1");
    command
}

fn path(path: &Path) -> &str {
    path.to_str().expect("ordinary fixture path is UTF-8")
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-commit-message-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("scratch root is exclusively created");
        git_repository::initialize(&root);
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
