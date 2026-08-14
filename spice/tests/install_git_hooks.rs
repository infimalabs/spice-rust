#![cfg(unix)]

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use spice::install::{
    InstallError, InstallPreview, apply, installed_binary, installed_git_hooks, plan,
};
use spice::repository::Repository;
use wire::Outcome;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn generated_hooks_exec_the_absolute_installed_spice_with_exact_process_contracts() {
    let fixture = Fixture::special();
    let repository = fixture.repository();
    let source = fixture.executable(
        "fixture-spice",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$@\" > \"$SPICE_HOOK_ARGV_LOG\"\n",
            ": > \"$SPICE_HOOK_STDIN_LOG\"\n",
            "while IFS= read -r line || [ -n \"$line\" ]; do\n",
            "  printf '%s\\n' \"$line\" >> \"$SPICE_HOOK_STDIN_LOG\"\n",
            "done\n",
            "exit \"${SPICE_HOOK_EXIT_CODE:-0}\"\n",
        )
        .as_bytes(),
    );
    let planned = plan(&repository, Some(&source)).expect("special-path install plans");
    apply(&repository, Some(&source), &planned.digest().unwrap()).expect("install applies");

    let hooks = installed_git_hooks(&repository);
    for (name, command) in [
        ("pre-commit", "pre-commit"),
        ("commit-msg", "commit-msg"),
        ("reference-transaction", "reference-transaction"),
    ] {
        assert_launcher(&hooks.join(name), command);
    }

    assert_hook(
        &fixture,
        &hooks.join("pre-commit"),
        &["commit arg", "quote'arg"],
        b"pre-commit input\n",
        b"gate\npre-commit\ncommit arg\nquote'arg\n",
    );
    assert_hook(
        &fixture,
        &hooks.join("commit-msg"),
        &["message file with spaces"],
        b"commit-message input\n",
        b"gate\ncommit-msg\nmessage file with spaces\n",
    );
    assert_hook(
        &fixture,
        &hooks.join("reference-transaction"),
        &["prepared"],
        b"old new refs/heads/main\n",
        b"gate\nreference-transaction\nprepared\n",
    );

    let propagated = invoke_hook(&fixture, &hooks.join("pre-commit"), &[], b"", "29");
    assert_eq!(propagated.status.code(), Some(29));

    fs::remove_file(installed_binary(&repository)).expect("remove installed fixture Spice");
    let missing = invoke_hook(
        &fixture,
        &hooks.join("reference-transaction"),
        &["prepared"],
        b"",
        "0",
    );
    assert!(!missing.status.success());
    assert!(!missing.stderr.is_empty());
}

#[test]
fn a_line_separator_in_the_git_common_path_refuses_without_mutation() {
    let fixture = Fixture::line_separator();
    let repository = fixture.repository();
    let source = fixture.executable("fixture-spice", b"#!/bin/sh\nexit 0\n");
    let before_exclude = fs::read(fixture.root.join(".git/info/exclude")).unwrap();

    let error = plan(&repository, Some(&source)).expect_err("newline path must refuse planning");
    assert!(matches!(
        error,
        InstallError::HookPathUnrepresentable {
            reason: "contains a line separator",
            ..
        }
    ));
    assert!(!fixture.root.join(".git/.spice-install").exists());
    assert!(!fixture.root.join(".git/.spice-rs").exists());
    assert_eq!(
        fs::read(fixture.root.join(".git/info/exclude")).unwrap(),
        before_exclude
    );

    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(["--json", "install"])
        .current_dir(&fixture.root)
        .env_remove(steer::hook::INBOX)
        .output()
        .expect("launch install preview in newline path");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let outcome: Outcome<InstallPreview> = wire::decode(&output.stderr).expect("refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("unrenderable hook path must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "install::hook-path-unrepresentable");
    let diagnostic = refusal
        .diagnostic()
        .expect("path refusal carries diagnostics");
    assert!(
        diagnostic
            .detail()
            .is_some_and(|detail| detail.contains("contains a line separator"))
    );
    assert!(!fixture.root.join(".git/.spice-install").exists());
    assert!(!fixture.root.join(".git/.spice-rs").exists());
}

fn assert_launcher(path: &Path, command: &str) {
    let bytes = fs::read(path).expect("read generated hook");
    let text = std::str::from_utf8(&bytes).expect("hook is UTF-8");
    assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 2);
    assert_eq!(text.lines().count(), 2);
    assert_eq!(text.lines().next(), Some("#!/bin/sh -eu"));
    assert!(text.lines().nth(1).is_some_and(
        |line| line.starts_with("exec '") && line.ends_with(&format!(" gate {command} \"$@\""))
    ));
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o755
    );
    for forbidden in [
        "cargo",
        "xtask",
        "git rev-parse",
        "dirname",
        "basename",
        "/usr/bin/env",
        "$(",
        "PATH=",
    ] {
        assert!(
            !text.contains(forbidden),
            "hook contains {forbidden:?}: {text}"
        );
    }
}

fn assert_hook(
    fixture: &Fixture,
    hook: &Path,
    arguments: &[&str],
    stdin: &[u8],
    expected_argv: &[u8],
) {
    let output = invoke_hook(fixture, hook, arguments, stdin, "0");
    assert!(
        output.status.success(),
        "hook failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(fs::read(fixture.argv_log()).unwrap(), expected_argv);
    assert_eq!(fs::read(fixture.stdin_log()).unwrap(), stdin);
}

fn invoke_hook(
    fixture: &Fixture,
    hook: &Path,
    arguments: &[&str],
    stdin: &[u8],
    exit_code: &str,
) -> Output {
    let mut child = Command::new(hook)
        .args(arguments)
        .env("SPICE_HOOK_ARGV_LOG", fixture.argv_log()) // env-policy: allow
        .env("SPICE_HOOK_STDIN_LOG", fixture.stdin_log()) // env-policy: allow
        .env("SPICE_HOOK_EXIT_CODE", exit_code) // env-policy: allow
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch generated hook");
    child
        .stdin
        .take()
        .expect("hook stdin is piped")
        .write_all(stdin)
        .expect("write inherited hook stdin");
    child.wait_with_output().expect("collect generated hook")
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn special() -> Self {
        Self::new("space tab\tquote'$`\\hook")
    }

    fn line_separator() -> Self {
        Self::new("line\nseparator")
    }

    fn new(label: &str) -> Self {
        for _ in 0..100 {
            let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "spice-install-hooks-{label}-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&root) {
                Ok(()) => {
                    let initialized = Command::new("git")
                        .args(["init", "-q", "-b", "main"])
                        .current_dir(&root)
                        .status()
                        .expect("launch hook fixture Git initialization");
                    assert!(initialized.success());
                    fs::create_dir_all(root.join(".git/info")).unwrap();
                    fs::write(root.join(".git/info/exclude"), b"fixture exclude\n").unwrap();
                    return Self { root };
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create fixture {root:?}: {error}"),
            }
        }
        panic!("no unique hook fixture path was available");
    }

    fn repository(&self) -> Repository {
        Repository::discover(&self.root).expect("fixture repository discovers")
    }

    fn executable(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, bytes).expect("write fixture executable");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("set fixture executable mode");
        path
    }

    fn argv_log(&self) -> PathBuf {
        self.root.join("argv.log")
    }

    fn stdin_log(&self) -> PathBuf {
        self.root.join("stdin.log")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root)
            && !std::thread::panicking()
        {
            panic!("remove fixture {:?}: {error}", self.root);
        }
    }
}
