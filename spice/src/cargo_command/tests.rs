use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;

/// The variables native Cargo receives from the wrapper, in recorded order.
const FORWARDED: [&str; 7] = [
    CARGO_HOME_VARIABLE,
    BOOTSTRAP_HOME_VARIABLE,
    NATIVE_CARGO_VARIABLE,
    NATIVE_RUSTC_VARIABLE,
    NATIVE_RUSTDOC_VARIABLE,
    RUSTC_VARIABLE,
    RUSTDOC_VARIABLE,
];

/// How long a fixture waits for its child to reach a stated point.
const SETTLE: Duration = Duration::from_secs(10);

/// A private toolchain whose `cargo` records what it was launched with as
/// files in its working directory, or with `linger` waits to be signalled.
struct Toolchain {
    root: PathBuf,
    host: Host,
}

impl Toolchain {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-cargo-boundary-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        for directory in ["bin", "bootstrap", "home", "work"] {
            fs::create_dir_all(root.join(directory)).expect("fixture directory creates");
        }
        let root = fs::canonicalize(root).expect("fixture root resolves");
        let recorded = FORWARDED
            .iter()
            .map(|name| format!("\"${{{name}}}\""))
            .collect::<Vec<_>>()
            .join(" ");
        let cargo = format!(
            r#"#!/bin/sh
if [ "$1" = linger ]; then
    trap 'printf parent > relayed-parent; exit 143' TERM
    sh -c 'trap "printf grandchild > relayed-grandchild; exit 0" TERM
        : > grandchild-ready
        while :; do sleep 0.05; done' > /dev/null 2>&1 &
    while [ ! -e grandchild-ready ]; do sleep 0.01; done
    : > ready
    while :; do sleep 0.05; done
fi
for argument in "$@"; do printf '%s\0' "$argument"; done > observed-arguments
printf '%s\0' {recorded} > observed-environment
"#
        );
        for (tool, script) in [
            ("cargo", cargo.as_str()),
            ("rustc", "#!/bin/sh\n"),
            ("rustdoc", "#!/bin/sh\n"),
        ] {
            let path = root.join("bin").join(tool);
            fs::write(&path, script).expect("fixture tool writes");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("fixture tool is executable");
        }
        let host = Host::new(
            root.join("bootstrap"),
            root.join("bin/cargo"),
            root.join("bin/rustc"),
            root.join("bin/rustdoc"),
        )
        .expect("fixture toolchain is admitted");
        Self { root, host }
    }

    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    /// Launch `invocation` exactly as `run_inner` does after store preparation.
    fn run(
        &self,
        invocation: &Invocation,
        relay: impl FnOnce(&Child) -> io::Result<Option<SignalRelay>>,
    ) -> (ExitStatus, String) {
        let report = Arc::new(Mutex::new(Vec::new()));
        let mut command = native_command(
            self.host.cargo(),
            &self.root.join("home"),
            &self.host,
            &self.work(),
            &invocation.arguments,
        );
        let settings = invocation.settings(report.clone(), self.root.join("captures"));
        let status = capture::run(&mut command, invocation.stdout, &settings, relay)
            .expect("fixture Cargo runs");
        let report =
            String::from_utf8(report.lock().expect("report lock").clone()).expect("UTF-8 report");
        (status, report)
    }

    fn observed(&self, name: &str) -> Vec<Vec<u8>> {
        let bytes = fs::read(self.work().join(name)).expect("fixture Cargo recorded its launch");
        bytes
            .split(|byte| *byte == 0)
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>()
            .split_last()
            .map(|(_, fields)| fields.to_vec())
            .unwrap_or_default()
    }

    fn expected_environment(&self) -> Vec<Vec<u8>> {
        [
            self.root.join("home").as_path(),
            self.host.bootstrap_home(),
            self.host.cargo(),
            self.host.rustc(),
            self.host.rustdoc(),
            self.host.rustc(),
            self.host.rustdoc(),
        ]
        .iter()
        .map(|path| path.as_os_str().as_bytes().to_vec())
        .collect()
    }
}

impl Drop for Toolchain {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn parse(arguments: &[&OsStr]) -> clap::ArgMatches {
    crate::cli::command()
        .try_get_matches_from(arguments)
        .expect("the generated parser admits the invocation")
}

fn eventually(path: &Path) -> bool {
    let deadline = Instant::now() + SETTLE;
    while !path.exists() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

#[test]
fn dev_cargo_forwards_every_argument_after_the_separator_and_the_private_environment() {
    let toolchain = Toolchain::new();
    let non_utf8 = OsStr::from_bytes(b"\xff");
    let matches = parse(&[
        OsStr::new("spice"),
        OsStr::new("dev"),
        OsStr::new("cargo"),
        OsStr::new("--verbose"),
        OsStr::new("--"),
        OsStr::new("test"),
        OsStr::new("--verbose"),
        OsStr::new("a b"),
        OsStr::new(""),
        non_utf8,
        OsStr::new("--"),
        OsStr::new("--nocapture"),
    ]);
    let invocation = cargo_invocation(crate::cli::selected_matches(&matches));
    assert!(
        invocation.verbose,
        "a flag before the separator is the wrapper's"
    );

    let (status, report) = toolchain.run(&invocation, relay);

    assert!(status.success(), "{report}");
    let expected: [&[u8]; 7] = [
        b"test",
        b"--verbose",
        b"a b",
        b"",
        b"\xff",
        b"--",
        b"--nocapture",
    ];
    assert_eq!(toolchain.observed("observed-arguments"), expected);
    assert_eq!(
        toolchain.observed("observed-environment"),
        toolchain.expected_environment()
    );
}

#[test]
fn dev_xtask_runs_the_locked_task_binary_with_its_arguments_verbatim() {
    let toolchain = Toolchain::new();
    let matches = parse(&[
        OsStr::new("spice"),
        OsStr::new("dev"),
        OsStr::new("xtask"),
        OsStr::new("--"),
        OsStr::new("verify"),
        OsStr::new("--package"),
        OsStr::new("spice"),
    ]);
    let invocation = xtask_invocation(crate::cli::selected_matches(&matches));
    assert!(!invocation.verbose);

    let (status, report) = toolchain.run(&invocation, relay);

    assert!(status.success(), "{report}");
    let expected: [&[u8]; 10] = [
        b"run",
        b"--locked",
        b"--package",
        b"xtask",
        b"--bin",
        b"xtask",
        b"--",
        b"verify",
        b"--package",
        b"spice",
    ];
    assert_eq!(toolchain.observed("observed-arguments"), expected);
}

/// A shell deadline terminates the wrapper; the relay must carry that signal
/// to Cargo's whole process group, and the wrapper reports Cargo's own exit.
///
/// The signal is raised at this test process only once the relay is installed
/// and the child has armed its traps, so the relay's handler receives it.
#[test]
fn a_terminated_wrapper_relays_the_signal_to_the_whole_cargo_process_group() {
    let toolchain = Toolchain::new();
    let invocation = Invocation {
        arguments: vec![OsString::from("linger")],
        surface: "dev cargo",
        identity: "cargo linger".to_owned(),
        stdout: Stdout::Capture,
        verbose: false,
    };
    let ready = toolchain.work().join("ready");

    let (status, report) = toolchain.run(&invocation, |child| {
        let guard = relay(child)?;
        std::thread::spawn(move || {
            if eventually(&ready) {
                let _ = rustix::process::kill_process(
                    rustix::process::getpid(),
                    rustix::process::Signal::TERM,
                );
            }
        });
        Ok(guard)
    });

    assert_eq!(status.code(), Some(143), "{report}");
    assert!(
        report.contains("cargo linger failed (exit 143)"),
        "{report}"
    );
    assert_eq!(
        fs::read(toolchain.work().join("relayed-parent")).expect("Cargo received the signal"),
        b"parent"
    );
    let grandchild = toolchain.work().join("relayed-grandchild");
    assert!(eventually(&grandchild), "{report}");
    assert_eq!(
        fs::read(grandchild).expect("descendant record reads"),
        b"grandchild"
    );
}

#[test]
fn developer_children_begin_outside_supervised_shell_state_and_may_opt_in() {
    let mut command = Command::new("cargo");
    command
        .env(steer::hook::BASH_ENV, "/fixture/bash-env")
        .env(steer::hook::ZDOTDIR, "/fixture/zdotdir")
        .env(vendor::OMP.thread_id_environment(), "fixture-omp");
    isolate_developer_environment(&mut command);

    for variable in spice::cargo_isolation::DEVELOPER_CHILD_EXCLUDED_VARIABLES {
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == variable && value.is_none()),
            "developer child does not inherit {variable}"
        );
    }

    command.env(vendor::OMP.thread_id_environment(), "owned-omp");
    assert!(command.get_envs().any(|(name, value)| {
        name == vendor::OMP.thread_id_environment() && value == Some(OsStr::new("owned-omp"))
    }));
}
