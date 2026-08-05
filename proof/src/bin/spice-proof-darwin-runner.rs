//! Repository-owned Darwin implementation of the native proof-runner protocol.

#![forbid(unsafe_code)]

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};

use proof::Battery;
use proof::challenge::{Challenge, ChallengeError};
use proof::native::{ATTESTATION_PREFIX, ISOLATION_CLAIMS, PROTOCOL};
use serde::Deserialize;
use wire::{Digest, Domain, IdentError};

const VENDOR_RELATIVE: &str = "target/proof-native-darwin/vendor";

/// Every pin this runner was sealed with, as `dist/proof-runner/build.sh` wrote
/// it.
///
/// The pins arrive as one compiled document rather than one variable each so
/// the runner's own bytes carry them: an executable built without the document
/// refuses instead of running unpinned, and a document edited after the build
/// is a different executable with a different identity. A workspace check-out
/// that never ran the build script still compiles, which is what keeps the
/// binary inside the ordinary workspace battery.
const PINS: Option<&str> = option_env!("SPICE_NATIVE_PINS"); // env-policy: allow

fn main() -> ExitCode {
    match dispatch(env::args_os().skip(1).collect()) {
        Ok(code) => u8::try_from(code)
            .ok()
            .map_or(ExitCode::FAILURE, ExitCode::from),
        Err(error) => {
            eprintln!("native Darwin proof runner refused: {error}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(arguments: Vec<OsString>) -> Result<i32, String> {
    match arguments.as_slice() {
        [operation, path] if operation == OsStr::new("closure-digest") => {
            println!(
                "{}",
                closure_digest(Path::new(path)).map_err(|error| error.to_string())?
            );
            Ok(0)
        }
        [operation, path] if operation == OsStr::new("tool-digest") => {
            println!(
                "{}",
                file_digest(Domain::GateTool, Path::new(path)).map_err(|error| error.to_string())?
            );
            Ok(0)
        }
        [operation, path] if operation == OsStr::new("runner-identity") => {
            println!(
                "{}",
                file_digest(Domain::ProofNativeRunner, Path::new(path))
                    .map_err(|error| error.to_string())?
            );
            Ok(0)
        }
        [operation] if operation == OsStr::new("cargo-home") => {
            println!("{}", ambient_cargo_home()?.display());
            Ok(0)
        }
        [protocol, identity, challenge, program, command @ ..]
            if protocol == OsStr::new(PROTOCOL) =>
        {
            execute_protocol(identity, challenge, program, command)
        }
        _ => Err(format!(
            "usage: {PROTOCOL} IDENTITY CHALLENGE cargo ARGUMENT..."
        )),
    }
}

fn execute_protocol(
    identity: &OsStr,
    challenge: &OsStr,
    program: &OsStr,
    arguments: &[OsString],
) -> Result<i32, String> {
    let identity: Digest = text(identity, "runner identity")?
        .parse()
        .map_err(|error: IdentError| error.to_string())?;
    let challenge: Challenge = text(challenge, "challenge")?
        .parse()
        .map_err(|error: ChallengeError| error.to_string())?;
    if program != OsStr::new("cargo") || !allowed_cargo_arguments(arguments) {
        return Err(format!(
            "command is outside the compiled proof battery: {:?} {:?}",
            program, arguments
        ));
    }

    validate_self(&identity)?;
    let pins = Pins::admit()?;
    pins.validate()?;
    let workspace = env::current_dir()
        .and_then(fs::canonicalize)
        .map_err(|error| format!("cannot resolve proof workspace: {error}"))?;
    validate_source(&workspace, &pins)?;
    validate_closure(&workspace, &pins)?;

    // Everything past this point is joined with the removal below, so a refusal
    // between creating the fresh directories and running the member takes them
    // with it: a leaked root would make the next run under the same challenge
    // refuse for a reason that has nothing to do with what it was asked.
    let temporary = Temporary::fresh(&challenge)?;
    let execution = prepared(
        arguments, &workspace, &pins, &temporary, &identity, &challenge,
    );
    let cleanup = temporary.remove();
    match (execution, cleanup) {
        (Ok(status), Ok(())) => Ok(status),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(execution), Err(cleanup)) => Err(format!(
            "{execution}; native proof cleanup also failed: {cleanup}"
        )),
    }
}

/// Establish the boundary, attest it, and run the member inside it.
fn prepared(
    arguments: &[OsString],
    workspace: &Path,
    pins: &Pins,
    temporary: &Temporary,
    identity: &Digest,
    challenge: &Challenge,
) -> Result<i32, String> {
    let profile = sandbox_profile(workspace, pins, temporary)?;
    temporary.write_cargo_config(workspace)?;

    eprintln!("{ATTESTATION_PREFIX}:runner={identity}:challenge={challenge}:{ISOLATION_CLAIMS}");
    io::stderr()
        .flush()
        .map_err(|error| format!("cannot flush runner attestation: {error}"))?;

    run_sandboxed(arguments, workspace, pins, temporary, &profile)
        .map(|status| status.code().unwrap_or(1))
}

/// Admit the toolchain probe and whatever the compiled battery names, only.
///
/// The members are read back from [`Battery::current`] rather than restated
/// here. The refusal above already says "outside the compiled proof battery",
/// and a second copy of the member list is the one thing that could make that
/// sentence false: a scope changed in the battery would keep passing the
/// unchanged copy, and the receipt would publish an identity for commands this
/// boundary never actually admitted.
///
/// An argument that is not UTF-8 is refused rather than compared. Every member
/// argument is a `String`, so no such argument can be one.
fn allowed_cargo_arguments(arguments: &[OsString]) -> bool {
    let Some(observed) = arguments
        .iter()
        .map(|argument| argument.to_str())
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    // Not a member: the sealed build probes the toolchain with it before any
    // member runs, so it is admitted beside them rather than declared as one.
    if observed == ["--version"] {
        return true;
    }
    Battery::current()
        .checks()
        .iter()
        .any(|check| check.program() == "cargo" && observed == check.arguments())
}

fn validate_self(expected: &Digest) -> Result<(), String> {
    let executable = env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|error| format!("cannot resolve runner executable: {error}"))?;
    let observed = file_digest(Domain::ProofNativeRunner, &executable)
        .map_err(|error| format!("cannot identify runner executable: {error}"))?;
    if &observed != expected {
        return Err(format!(
            "runner identity changed: expected {expected}, observed {observed}"
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pins {
    source_commit: String,
    vendor_digest: Digest,
    sdk_path: PathBuf,
    developer_dir: PathBuf,
    /// The warm Cargo caches the sealed build is denied, so they cannot stand
    /// in for the vendored closure.
    ///
    /// This is sealed rather than read back from the environment because the
    /// host launches the runner with no environment to read: naming the caches
    /// is the sealing host's statement about itself, and the runner refuses
    /// below if that statement stopped describing this machine.
    cargo_home: PathBuf,
    cargo: Tool,
    rustc: Tool,
    cargo_mutants: Tool,
    git: Tool,
    sandbox_exec: Tool,
}

impl Pins {
    fn admit() -> Result<Self, String> {
        let document = PINS.ok_or("runner was built without its pin document")?;
        toml::from_str(document)
            .map_err(|error| format!("runner pin document is malformed: {error}"))
    }

    fn tools(&self) -> [(&'static str, &Tool); 5] {
        [
            ("cargo", &self.cargo),
            ("rustc", &self.rustc),
            ("cargo-mutants", &self.cargo_mutants),
            ("git", &self.git),
            ("sandbox-exec", &self.sandbox_exec),
        ]
    }

    fn directories(&self) -> [(&'static str, &Path); 3] {
        [
            ("SDK", &self.sdk_path),
            ("developer directory", &self.developer_dir),
            ("Cargo home", &self.cargo_home),
        ]
    }

    /// Every path outside the workspace this run was sealed to reach.
    ///
    /// The two halves are the ones already declared above rather than a third
    /// list, so a pin added to the document arrives here without being
    /// remembered twice. What reads this is [`denied_parent`], which needs to
    /// know what a denial of the workspace's neighbourhood would take away.
    fn sealed_paths(&self) -> Vec<(&'static str, &Path)> {
        let mut paths = self.directories().to_vec();
        paths.extend(self.tools().map(|(name, tool)| (name, tool.path.as_path())));
        paths
    }

    fn validate(&self) -> Result<(), String> {
        if env::consts::OS != "macos" || env::consts::ARCH != "aarch64" {
            return Err(format!(
                "runner requires native aarch64 macOS, observed {} {}",
                env::consts::OS,
                env::consts::ARCH
            ));
        }
        for (name, tool) in self.tools() {
            tool.validate(name)?;
        }
        for (label, path) in self.directories() {
            if !path.is_dir() {
                return Err(format!(
                    "pinned {label} is not a directory: {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }
}

/// One executable this runner is sealed against.
///
/// The pin is the exact bytes at an exact absolute path. A tool cannot report
/// a version its bytes do not already determine, so there is nothing to ask it:
/// every pinned tool is exercised for real on the path that uses it -- `git`
/// reading the source, `sandbox-exec` establishing the boundary, and `cargo`
/// driving `rustc` or `cargo-mutants` inside it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tool {
    path: PathBuf,
    digest: Digest,
}

impl Tool {
    fn validate(&self, name: &str) -> Result<(), String> {
        if !self.path.is_absolute() {
            return Err(format!("pinned {name} path is not absolute"));
        }
        let canonical = fs::canonicalize(&self.path)
            .map_err(|error| format!("cannot resolve pinned {name}: {error}"))?;
        if canonical != self.path {
            return Err(format!(
                "pinned {name} path moved: expected {}, observed {}",
                self.path.display(),
                canonical.display()
            ));
        }
        let observed = file_digest(Domain::GateTool, &self.path)
            .map_err(|error| format!("cannot identify pinned {name}: {error}"))?;
        if observed != self.digest {
            return Err(format!(
                "pinned {name} changed: expected {}, observed {observed}",
                self.digest
            ));
        }
        Ok(())
    }
}

fn validate_source(workspace: &Path, pins: &Pins) -> Result<(), String> {
    let commit = git_one_line(
        &pins.git.path,
        workspace,
        ["rev-parse", "--verify", "HEAD^{commit}"],
    )?;
    if commit != pins.source_commit {
        return Err(format!(
            "source commit changed: expected {}, observed {}",
            pins.source_commit, commit
        ));
    }
    let tracked = git_status(
        &pins.git.path,
        workspace,
        ["diff-index", "--quiet", "HEAD", "--"],
    )?;
    if !tracked.success() {
        return Err("tracked source differs from the pinned commit".to_owned());
    }
    let untracked = git_text(
        &pins.git.path,
        workspace,
        ["ls-files", "--others", "--exclude-standard"],
    )?;
    if let Some(path) = untracked.lines().find(|path| {
        !path.starts_with("dist/stage/objects/") && !path.starts_with("dist/stage/targets/")
    }) {
        return Err(format!(
            "untracked source is outside release output: {path}"
        ));
    }
    Ok(())
}

fn validate_closure(workspace: &Path, pins: &Pins) -> Result<(), String> {
    let vendor = workspace.join(VENDOR_RELATIVE);
    let observed = closure_digest(&vendor).map_err(|error| error.to_string())?;
    if observed != pins.vendor_digest {
        return Err(format!(
            "vendor closure changed: expected {}, observed {}",
            pins.vendor_digest, observed
        ));
    }
    Ok(())
}

fn git_one_line<const N: usize>(
    git: &Path,
    workspace: &Path,
    arguments: [&str; N],
) -> Result<String, String> {
    let bytes = git_bytes(git, workspace, arguments)?;
    one_line(&bytes, "Git source observation")
}

fn git_text<const N: usize>(
    git: &Path,
    workspace: &Path,
    arguments: [&str; N],
) -> Result<String, String> {
    let bytes = git_bytes(git, workspace, arguments)?;
    std::str::from_utf8(&bytes)
        .map(|value| value.trim_end_matches(['\r', '\n']).to_owned())
        .map_err(|_| "Git source observation is not UTF-8".to_owned())
}

fn git_bytes<const N: usize>(
    git: &Path,
    workspace: &Path,
    arguments: [&str; N],
) -> Result<Vec<u8>, String> {
    let output = Command::new(git)
        .args(arguments)
        .env_clear()
        .current_dir(workspace)
        .output()
        .map_err(|error| format!("cannot inspect pinned source: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "pinned source inspection failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim_end()
        ));
    }
    Ok(output.stdout)
}

fn git_status<const N: usize>(
    git: &Path,
    workspace: &Path,
    arguments: [&str; N],
) -> Result<ExitStatus, String> {
    Command::new(git)
        .args(arguments)
        .env_clear()
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .map_err(|error| format!("cannot inspect pinned source: {error}"))
}

struct Temporary {
    root: PathBuf,
    cargo_home: PathBuf,
    cargo_target: PathBuf,
    home: PathBuf,
    /// Where the mutation member writes its `mutants.out`.
    ///
    /// The battery names one command per member and no locations, because the
    /// locations are what an isolation boundary owns: the workspace is read-only
    /// wherever the battery runs, so the boundary has to say where the writing
    /// goes, the same way it already does for the Cargo home and target.
    mutants: PathBuf,
    temporary: PathBuf,
}

impl Temporary {
    fn fresh(challenge: &Challenge) -> Result<Self, String> {
        let root = Path::new("/private/tmp").join(format!("spice-proof-native-{challenge}"));
        fs::create_dir(&root).map_err(|error| {
            format!(
                "cannot create fresh native proof root {}: {error}",
                root.display()
            )
        })?;
        let instance = Self {
            cargo_home: root.join("cargo-home"),
            cargo_target: root.join("cargo-target"),
            home: root.join("home"),
            mutants: root.join("mutants"),
            temporary: root.join("tmp"),
            root,
        };
        for path in [
            &instance.cargo_home,
            &instance.cargo_target,
            &instance.home,
            &instance.mutants,
            &instance.temporary,
        ] {
            fs::create_dir(path).map_err(|error| {
                format!("cannot create fresh proof path {}: {error}", path.display())
            })?;
        }
        Ok(instance)
    }

    fn write_cargo_config(&self, workspace: &Path) -> Result<(), String> {
        let vendor = toml_string(&workspace.join(VENDOR_RELATIVE))?;
        let config = format!(
            concat!(
                "[source.crates-io]\n",
                "replace-with = \"vendored-sources\"\n\n",
                "[source.vendored-sources]\n",
                "directory = \"{}\"\n\n",
                "[net]\n",
                "offline = true\n"
            ),
            vendor
        );
        fs::write(self.cargo_home.join("config.toml"), config)
            .map_err(|error| format!("cannot write fresh Cargo configuration: {error}"))
    }

    fn remove(self) -> Result<(), String> {
        fs::remove_dir_all(&self.root).map_err(|error| {
            format!(
                "cannot remove native proof root {}: {error}",
                self.root.display()
            )
        })
    }
}

/// Where the sealing host keeps the warm Cargo caches the sealed build is denied.
///
/// Only sealing asks this, and it asks the runner rather than the shell so
/// every pin the sealed build enforces was computed by the one implementation
/// that enforces it. The sealed runner reads the answer back from its pins,
/// because the host launches it with no environment left to read.
#[expect(
    clippy::disallowed_methods,
    reason = "the runner rim owns ambient acquisition of the caches it denies"
)]
fn ambient_cargo_home() -> Result<PathBuf, String> {
    if let Some(home) = env::var_os("CARGO_HOME") {
        return Ok(PathBuf::from(home));
    }
    env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".cargo"))
        .ok_or_else(|| "the sealing environment names no Cargo home".to_owned())
}

/// The host dependency roots a sealed build is denied.
///
/// Every entry is a package manager prefix or the third-party framework
/// directory, which is where a `-sys` crate's build script goes looking when it
/// probes for a library to link. The container half cannot see any of them
/// because it runs where the host filesystem is absent, so a build that found
/// one here would link a dependency that half could never link, and both
/// receipts would still read `host-dependencies=denied`. The pinned SDK,
/// developer directory and toolchain are deliberately absent from this list:
/// the build is meant to read those, and `/Library/Developer` stays readable
/// precisely because the denial names `/Library/Frameworks` and not `/Library`.
const HOST_DEPENDENCY_ROOTS: [&str; 6] = [
    "/Library/Frameworks",
    "/nix",
    "/opt/homebrew",
    "/opt/local",
    "/sw",
    "/usr/local",
];

/// The roots above as one denial in the sandbox dialect.
///
/// Spelled here rather than inside the profile literal so the fixture proving
/// the denial bites can run the bytes this runner ships instead of a restated
/// copy of them, which is the one difference a restatement could not show.
fn host_dependency_denials() -> String {
    let mut denial = "(deny file-read*".to_owned();
    for root in HOST_DEPENDENCY_ROOTS {
        let _ = write!(denial, " (subpath \"{root}\")");
    }
    denial.push_str(")\n");
    denial
}

/// The directory whose contents the boundary takes away, or why it cannot.
///
/// `(allow default)` leaves readable everything no rule names, and on a machine
/// that keeps its checkouts together that is every other checkout: ten peer
/// worktrees sit beside this one here. A sealed build reading a neighbour's tree
/// would be reading source no receipt names and no pin covers, so the directory
/// holding the workspace is denied and only the paths this profile itself names
/// are given back.
///
/// A pinned path under that parent refuses the run instead, because the denial
/// would take it with nothing to allow it back from and the build would then
/// fail somewhere far from the cause. The rustup toolchain root is not pinned by
/// name and does not need to be: `dist/proof-runner/build.sh` seals the compiler
/// `rustup which rustc` resolves, and that binary lives inside the root -- here
/// `rustc --print sysroot` answers exactly the grandparent of the pinned path --
/// so a toolchain under the parent brings the `rustc` pin under it and refuses.
///
/// A pin below the workspace is exempt, because the workspace is allowed back
/// whole. Nothing else is: a path is compared as the pin document spells it,
/// which for a tool is the canonical path [`Tool::validate`] already insisted on.
fn denied_parent<'a>(workspace: &'a Path, pins: &Pins) -> Result<&'a Path, String> {
    let parent = workspace.parent().ok_or_else(|| {
        format!(
            "proof workspace {} has no parent, so the boundary has no neighbourhood to take",
            workspace.display()
        )
    })?;
    for (label, path) in pins.sealed_paths() {
        if path.starts_with(parent) && !path.starts_with(workspace) {
            return Err(format!(
                "pinned {label} {} sits beside the proof workspace, under {}, which the \
                 boundary denies so no neighbouring checkout is readable; the sealed build \
                 would lose it with nothing to allow it back from",
                path.display(),
                parent.display()
            ));
        }
    }
    Ok(parent)
}

/// Where this workspace's repository actually lives.
///
/// A linked worktree keeps it beside the workspace rather than inside it: every
/// lane of this repository is one, and answers here with a sibling directory.
/// That is why the denial above cannot be the whole rule.
fn git_common_dir(workspace: &Path, git: &Path) -> Result<PathBuf, String> {
    let common = git_one_line(
        git,
        workspace,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    fs::canonicalize(common)
        .map_err(|error| format!("cannot resolve Git common directory: {error}"))
}

/// The boundary the attestation above claims, written in the sandbox dialect.
///
/// Loopback is admitted because the battery's own corpus binds it: the members
/// stand up servers on `127.0.0.1` and talk to them, so a boundary that denied
/// loopback would not be isolating the proof, it would be deleting it. What the
/// proof needs denied is a route off this host, which is the same boundary the
/// container half establishes with a network namespace that has nothing but a
/// loopback interface in it.
///
/// `(allow default)` opens the profile, so every path this build reads is
/// readable unless a later rule takes it away -- the dialect resolves a path by
/// its last matching rule. That is why [`HOST_DEPENDENCY_ROOTS`] is denied
/// below rather than an allow-list of everything a Rust toolchain touches being
/// enumerated here, which would be a second and weaker copy of the toolchain's
/// own knowledge of itself. It also means this profile still denies less than
/// the container half does, which has no host filesystem to deny.
///
/// The one allow-list here is the pair [`denied_parent`] forces: the workspace's
/// neighbourhood is denied, then the workspace, its repository and the run's own
/// scratch root are named back. That pair sits above every later denial on
/// purpose. Read downward, nothing below it is weakened -- the warm Cargo caches
/// and the host dependency roots still resolve to their denials even for a
/// workspace that happens to contain one -- so what the pair adds is subtraction
/// only, which is the whole of what it is for.
fn sandbox_profile(workspace: &Path, pins: &Pins, temporary: &Temporary) -> Result<String, String> {
    let parent = denied_parent(workspace, pins)?;
    let git_common = git_common_dir(workspace, &pins.git.path)?;
    let host_registry = pins.cargo_home.join("registry");
    let host_git = pins.cargo_home.join("git");
    Ok(format!(
        concat!(
            "(version 1)\n",
            "(allow default)\n",
            "(deny network*)\n",
            "(allow network-bind (local ip \"localhost:*\"))\n",
            "(allow network-inbound (local ip \"localhost:*\"))\n",
            "(allow network-outbound (remote ip \"localhost:*\"))\n",
            "(deny file-write* (subpath \"{}\") (subpath \"{}\"))\n",
            "(deny file-read* (subpath \"{}\"))\n",
            "(allow file-read* (subpath \"{}\") (subpath \"{}\") (subpath \"{}\"))\n",
            "(deny file-read* (subpath \"{}\") (subpath \"{}\"))\n",
            "{}",
            "(allow file-write* (subpath \"{}\"))\n"
        ),
        sandbox_string(workspace)?,
        sandbox_string(&git_common)?,
        sandbox_string(parent)?,
        sandbox_string(workspace)?,
        sandbox_string(&git_common)?,
        sandbox_string(&temporary.root)?,
        sandbox_string(&host_registry)?,
        sandbox_string(&host_git)?,
        host_dependency_denials(),
        sandbox_string(&temporary.root)?,
    ))
}

fn run_sandboxed(
    arguments: &[OsString],
    workspace: &Path,
    pins: &Pins,
    temporary: &Temporary,
    profile: &str,
) -> Result<ExitStatus, String> {
    let cargo_dir = pins
        .cargo
        .path
        .parent()
        .ok_or_else(|| "pinned Cargo has no parent directory".to_owned())?;
    let mutants_dir = pins
        .cargo_mutants
        .path
        .parent()
        .ok_or_else(|| "pinned cargo-mutants has no parent directory".to_owned())?;
    let path = env::join_paths([
        cargo_dir,
        mutants_dir,
        Path::new("/usr/bin"),
        Path::new("/bin"),
    ])
    .map_err(|error| format!("cannot construct sealed PATH: {error}"))?;
    Command::new(&pins.sandbox_exec.path)
        .arg("-p")
        .arg(profile)
        .arg(&pins.cargo.path)
        .args(arguments)
        .env_clear()
        .env("CARGO", &pins.cargo.path)
        .env("CARGO_HOME", &temporary.cargo_home)
        .env("CARGO_TARGET_DIR", &temporary.cargo_target)
        .env("CARGO_BUILD_BUILD_DIR", temporary.root.join("build"))
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_MUTANTS_OUTPUT", &temporary.mutants)
        .env("CARGO_NET_OFFLINE", "true")
        .env("DEVELOPER_DIR", &pins.developer_dir)
        .env("HOME", &temporary.home)
        .env("NO_COLOR", "true")
        .env("PATH", path)
        .env("RUSTC", &pins.rustc.path)
        .env("RUSTC_WRAPPER", "")
        .env("RUSTC_WORKSPACE_WRAPPER", "")
        .env("SDKROOT", &pins.sdk_path)
        .env("TMPDIR", &temporary.temporary)
        .current_dir(workspace)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| format!("cannot execute pinned sandbox boundary: {error}"))
}

/// Digest of what the vendored closure contains.
///
/// Each file is digested under its own domain and the listing of those digests
/// is digested under another, the same framing a gate jurisdiction identifies
/// its tree with, so a file whose bytes happen to spell a listing cannot be
/// mistaken for one.
fn closure_digest(root: &Path) -> io::Result<Digest> {
    let root = fs::canonicalize(root)?;
    let mut files = Vec::new();
    collect_files(&root, &root, &mut files)?;
    files.sort();
    let mut listing = String::new();
    for relative in files {
        let spelling = relative.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "vendor path is not UTF-8")
        })?;
        let bytes = fs::read(root.join(&relative))?;
        let _ = writeln!(
            listing,
            "{} {spelling}",
            Digest::compute(Domain::ProofNativeClosureFile, &bytes)
        );
    }
    Ok(Digest::compute(
        Domain::ProofNativeClosure,
        listing.as_bytes(),
    ))
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("vendor closure contains symlink {}", path.display()),
            ));
        }
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(
                path.strip_prefix(root)
                    .expect("recursive vendor path remains below root")
                    .to_path_buf(),
            );
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("vendor closure contains special file {}", path.display()),
            ));
        }
    }
    Ok(())
}

fn file_digest(domain: Domain, path: &Path) -> io::Result<Digest> {
    fs::read(path).map(|bytes| Digest::compute(domain, &bytes))
}

fn text<'a>(value: &'a OsStr, label: &str) -> Result<&'a str, String> {
    value
        .to_str()
        .ok_or_else(|| format!("{label} is not UTF-8"))
}

fn one_line(bytes: &[u8], label: &str) -> Result<String, String> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| format!("{label} output is not UTF-8"))?
        .trim_end_matches(['\r', '\n']);
    if value.is_empty() || value.contains(['\r', '\n']) {
        return Err(format!("{label} output is not one nonempty line"));
    }
    Ok(value.to_owned())
}

fn toml_string(path: &Path) -> Result<String, String> {
    escaped_path(path, "TOML")
}

fn sandbox_string(path: &Path) -> Result<String, String> {
    escaped_path(path, "sandbox profile")
}

fn escaped_path(path: &Path, label: &str) -> Result<String, String> {
    let spelling = path
        .to_str()
        .ok_or_else(|| format!("{label} path is not UTF-8: {}", path.display()))?;
    if spelling.contains(['"', '\\', '\n', '\r']) {
        return Err(format!(
            "{label} path contains an unsupported character: {}",
            path.display()
        ));
    }
    Ok(spelling.to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Digest, Domain, Pins, Temporary, Tool, allowed_cargo_arguments};

    /// A pin naming a path and no bytes worth checking.
    ///
    /// Nothing below validates a pin: what these fixtures exercise composes the
    /// profile, which reads paths out of the document and never opens them.
    fn pinned(path: &str) -> Tool {
        Tool {
            path: PathBuf::from(path),
            digest: Digest::compute(Domain::GateTool, b""),
        }
    }

    /// A pin document written here rather than admitted.
    ///
    /// A workspace check-out carries none, and the two tools these fixtures
    /// actually run are this machine's for the same reason. Every other path is
    /// absent on purpose, and all of them sit outside any workspace parent used
    /// below, so a refusal from [`denied_parent`] is one the caller asked for.
    fn fixture_pins() -> Pins {
        Pins {
            source_commit: "0".repeat(40),
            vendor_digest: Digest::compute(Domain::ProofNativeClosure, b""),
            sdk_path: PathBuf::from("/nonexistent/sdk"),
            developer_dir: PathBuf::from("/nonexistent/developer"),
            cargo_home: PathBuf::from("/nonexistent/cargo-home"),
            cargo: pinned("/nonexistent/cargo"),
            rustc: pinned("/nonexistent/rustc"),
            cargo_mutants: pinned("/nonexistent/cargo-mutants"),
            git: pinned("/usr/bin/git"),
            sandbox_exec: pinned("/usr/bin/sandbox-exec"),
        }
    }

    /// The scratch root layout, spelled without creating any of it.
    fn fixture_temporary() -> Temporary {
        let root = PathBuf::from("/private/tmp/spice-proof-native-profile-fixture");
        Temporary {
            cargo_home: root.join("cargo-home"),
            cargo_target: root.join("cargo-target"),
            home: root.join("home"),
            mutants: root.join("mutants"),
            temporary: root.join("tmp"),
            root,
        }
    }

    /// The checkout this test binary was compiled from.
    ///
    /// The repository root, not the crate directory: the sealed run is handed
    /// the workspace, and the neighbourhood the profile denies is the one the
    /// root sits in. On this repository that is a directory of peer worktrees.
    fn repository_workspace() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the proof crate sits inside the repository it is proving")
    }

    /// The bytes a sealed run of this checkout would be sandboxed with.
    #[cfg(target_os = "macos")]
    fn shipped_profile() -> String {
        super::sandbox_profile(
            repository_workspace(),
            &fixture_pins(),
            &fixture_temporary(),
        )
        .expect("the sealed profile composes for a workspace check-out")
    }

    /// A profile that denies what the shipped one denies and gives nothing back.
    ///
    /// Cut from the shipped bytes rather than written out, so what it measures
    /// is the shipped allow going missing and not a second author's idea of the
    /// rest of the profile.
    #[cfg(target_os = "macos")]
    fn without_the_allow(profile: &str) -> String {
        let mut kept = String::new();
        let mut dropped = 0_usize;
        for line in profile.lines() {
            if line.starts_with("(allow file-read*") {
                dropped += 1;
                continue;
            }
            kept.push_str(line);
            kept.push('\n');
        }
        assert_eq!(
            dropped, 1,
            "the profile names its readable paths in one allow, and this cut {dropped}:\n{profile}",
        );
        kept
    }

    /// A profile that denies nothing, so a path unreadable under it was already
    /// unreachable for a reason no rule here can claim.
    #[cfg(target_os = "macos")]
    const OPEN: &str = "(version 1)\n(allow default)\n";

    /// The attestation claims host dependencies are denied, and until the roots
    /// below were named this profile denied none of them. The claim is only
    /// worth its bytes if a library under one of them is actually unreachable,
    /// so this runs the shipped denial through `sandbox-exec` and looks. The
    /// binary here is this machine's rather than the pinned one, because a
    /// workspace check-out has no pin document to read a path out of; what is
    /// being measured is the dialect's answer, which does not vary by caller.
    ///
    /// Each root is measured twice, because a root that is unreadable for any
    /// other reason -- absent, or unreadable to this user -- would refuse under
    /// the denial too and prove nothing. The open profile establishes that the
    /// denial is what took it away.
    ///
    /// The inventory is written out here rather than read from
    /// `HOST_DEPENDENCY_ROOTS`, because a test that asks the subject which
    /// roots to check loses the one mutation that matters: dropping a root
    /// would drop its own check with it and pass. Measured -- deriving the list
    /// from the constant, replacing `/opt/homebrew` with an absent path left
    /// all tests green.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_host_dependency_root_is_unreachable_inside_the_boundary() {
        use super::host_dependency_denials;

        const MUST_BE_UNREACHABLE: [&str; 6] = [
            "/Library/Frameworks",
            "/nix",
            "/opt/homebrew",
            "/opt/local",
            "/sw",
            "/usr/local",
        ];
        let denied = format!("{OPEN}{}", host_dependency_denials());

        let present = MUST_BE_UNREACHABLE
            .into_iter()
            .filter(|root| Path::new(root).is_dir())
            .collect::<Vec<_>>();
        assert!(
            !present.is_empty(),
            "no host dependency root exists on this machine, so nothing here could fail",
        );

        let mut libraries = 0_usize;
        for root in present {
            assert!(
                readable(OPEN, root),
                "{root} was already unreachable without the denial, so its refusal proves nothing",
            );
            assert!(
                !readable(&denied, root),
                "{root} stayed readable inside the boundary that attests host-dependencies=denied",
            );
            let Some(library) = first_file_under(Path::new(root)) else {
                continue;
            };
            let library = library.to_str().expect("a discovered path is UTF-8");
            assert!(
                contents_readable(OPEN, library),
                "{library} was already unreadable without the denial",
            );
            assert!(
                !contents_readable(&denied, library),
                "{library} could still be read inside the boundary",
            );
            libraries += 1;
        }
        assert!(
            libraries > 0,
            "every present root was empty, so no host library was measured",
        );

        for kept in ["/usr/lib", "/Library/Developer"] {
            if Path::new(kept).is_dir() {
                assert!(
                    readable(&denied, kept),
                    "the denial reached past the host dependency roots and took {kept}",
                );
            }
        }
    }

    /// The profile this runner ships carries the denial proved above.
    ///
    /// That proof composes its own profile out of [`host_dependency_denials`],
    /// so what it measures is the dialect, not the runner. [`sandbox_profile`]
    /// has one other caller and it is the sealed run, so nothing else can
    /// observe the bytes that actually seal a build. Measured -- deleting the
    /// denial from the profile left every test green, and the boundary would
    /// have denied nothing while still attesting `host-dependencies=denied`.
    ///
    /// Position is asserted as well as presence: the dialect resolves a path by
    /// its last matching rule, so a denial spelled above `(allow default)` is in
    /// the profile and does nothing.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_shipped_profile_denies_the_host_dependency_roots() {
        use super::host_dependency_denials;

        let profile = shipped_profile();
        let Some(denied) = profile.find(&host_dependency_denials()) else {
            panic!("the profile this runner ships denies no host dependency root:\n{profile}");
        };
        let opened = profile
            .find("(allow default)")
            .expect("the profile opens by allowing what it does not deny");
        assert!(
            opened < denied,
            "the denial is spelled above (allow default), which takes it back:\n{profile}",
        );
    }

    /// A checkout beside this one cannot be read inside the boundary.
    ///
    /// `(allow default)` left every peer worktree on this machine readable, and
    /// a sealed build that read one would be reading source no receipt names.
    /// The neighbour is measured twice for the reason the host dependency proof
    /// above measures twice: a directory unreadable for some other reason would
    /// refuse under the denial too and prove nothing about it. Metadata and
    /// bytes are both measured, because listing a directory only asks the
    /// boundary about metadata and source is read.
    ///
    /// The neighbour is discovered rather than named, so this asks the machine
    /// it runs on which checkout sits beside the workspace instead of asserting
    /// a lane's own layout.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_checkout_beside_the_workspace_is_unreachable_inside_the_boundary() {
        let workspace = repository_workspace();
        let profile = shipped_profile();
        let parent = workspace.parent().expect("the workspace sits somewhere");
        let repository = super::git_common_dir(workspace, Path::new("/usr/bin/git"))
            .expect("this check-out has a repository");

        let mut neighbours = std::fs::read_dir(parent)
            .expect("the directory holding the workspace is readable")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path != workspace && path != &repository && path.join(".git").exists())
            .collect::<Vec<_>>();
        neighbours.sort();
        let Some((neighbour, file)) = neighbours
            .iter()
            .find_map(|path| first_file_under(path).map(|file| (path, file)))
        else {
            panic!(
                "no checkout sits beside {}, so nothing here could fail",
                parent.display()
            );
        };
        let neighbour = neighbour.to_str().expect("a discovered path is UTF-8");
        let file = file.to_str().expect("a discovered path is UTF-8");

        assert!(
            readable(OPEN, neighbour),
            "{neighbour} was already unreachable, so its refusal proves nothing",
        );
        assert!(
            contents_readable(OPEN, file),
            "{file} was already unreadable, so its refusal proves nothing",
        );
        assert!(
            !readable(&profile, neighbour),
            "{neighbour} stayed listable inside the boundary this runner seals with",
        );
        assert!(
            !contents_readable(&profile, file),
            "{file} could still be read inside the boundary this runner seals with",
        );
    }

    /// The workspace and its repository survive the denial that takes the rest.
    ///
    /// A linked worktree keeps the repository beside the workspace rather than
    /// inside it, which is what makes the allow load-bearing here rather than
    /// decorative: measured on a lane of this repository, the common directory
    /// is a sibling of the workspace, and the denial would take it.
    ///
    /// Each path is measured against the shipped profile with its allow cut
    /// away, so what is proved is that the allow is holding the path up. A plain
    /// clone, whose repository sits inside the workspace, answers the same way
    /// for the same reason -- the cut takes the workspace with it.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_workspace_and_its_repository_stay_readable_inside_the_boundary() {
        let workspace = repository_workspace();
        let profile = shipped_profile();
        let cut = without_the_allow(&profile);
        let repository = super::git_common_dir(workspace, Path::new("/usr/bin/git"))
            .expect("this check-out has a repository");

        let manifest = workspace.join("Cargo.toml");
        let head = repository.join("HEAD");
        for path in [workspace, repository.as_path(), &manifest, &head] {
            let path = path.to_str().expect("a repository path is UTF-8");
            assert!(
                readable(&profile, path),
                "the boundary took {path}, which the sealed run is handed",
            );
            assert!(
                !readable(&cut, path),
                "{path} survives without the allow, so nothing here measures it",
            );
        }
        for file in [&manifest, &head] {
            let file = file.to_str().expect("a repository path is UTF-8");
            assert!(
                contents_readable(&profile, file),
                "the boundary allows {file} to be listed but not read",
            );
        }
    }

    /// A pin the denial would swallow refuses the run and says which one.
    ///
    /// The refusal is asserted by its words, not by being a refusal. This
    /// function has two ways to refuse and a bare `is_err` cannot tell them
    /// apart, so a check that named the wrong pin, or named none, would pass.
    ///
    /// Two different pins are put under the parent, because one case cannot
    /// show a check that stops after the entry it happens to name. The `rustc`
    /// case is the toolchain root: the pin document names no such root, and
    /// needs none, because the compiler it does name lives inside it.
    #[test]
    fn a_pin_beside_the_workspace_refuses_instead_of_being_denied() {
        use super::denied_parent;

        let workspace = Path::new("/fixture/workspace");
        assert_eq!(
            denied_parent(workspace, &fixture_pins()),
            Ok(Path::new("/fixture")),
            "a pin document naming nothing beside the workspace has nothing to refuse",
        );

        let mut sdk = fixture_pins();
        sdk.sdk_path = PathBuf::from("/fixture/sdk");
        let refusal =
            denied_parent(workspace, &sdk).expect_err("the SDK sits beside the workspace");
        for named in ["pinned SDK", "/fixture/sdk", "/fixture"] {
            assert!(
                refusal.contains(named),
                "the refusal does not name {named}: {refusal}"
            );
        }

        let mut toolchain = fixture_pins();
        toolchain.rustc = pinned("/fixture/toolchain/bin/rustc");
        let refusal =
            denied_parent(workspace, &toolchain).expect_err("the toolchain sits beside it too");
        for named in ["pinned rustc", "/fixture/toolchain/bin/rustc"] {
            assert!(
                refusal.contains(named),
                "the refusal does not name {named}: {refusal}"
            );
        }

        let mut inside = fixture_pins();
        inside.cargo_home = PathBuf::from("/fixture/workspace/.cargo");
        assert!(
            denied_parent(workspace, &inside).is_ok(),
            "a pin below the workspace is allowed back with it and refuses nothing",
        );

        let mut adjacent = fixture_pins();
        adjacent.sdk_path = PathBuf::from("/fixtureXX/sdk");
        assert!(
            denied_parent(workspace, &adjacent).is_ok(),
            "a directory whose name merely starts with the parent's is not under it",
        );

        assert!(
            denied_parent(Path::new("/"), &fixture_pins()).is_err(),
            "a workspace with no parent leaves the boundary nothing to take",
        );
    }

    /// Whether `sandbox-exec` lets a directory be listed under one profile.
    #[cfg(target_os = "macos")]
    fn readable(profile: &str, directory: &str) -> bool {
        sandboxed(profile, "/bin/ls", directory)
    }

    /// Whether `sandbox-exec` lets a file's bytes be read under one profile.
    ///
    /// Listing a directory only asks the boundary about metadata. A library is
    /// linked by reading it, so the claim is about this.
    #[cfg(target_os = "macos")]
    fn contents_readable(profile: &str, file: &str) -> bool {
        sandboxed(profile, "/bin/cat", file)
    }

    #[cfg(target_os = "macos")]
    fn sandboxed(profile: &str, tool: &str, path: &str) -> bool {
        std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", profile, tool, path])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("macOS carries sandbox-exec, which this runner is pinned against")
            .success()
    }

    /// The first regular file at or one level below a directory.
    ///
    /// Enough to put a real file under a denied root in front of the boundary
    /// without walking a whole prefix, and `None` rather than a panic when a
    /// root turns out to be empty, which the caller counts.
    ///
    /// The type comes from the directory entry, which does not follow symlinks,
    /// so a link is skipped rather than followed. That is deliberate: a link
    /// under a denied root can point outside it, and reading its target would
    /// measure a path the denial never named. `Path::is_file` would follow it.
    #[cfg(target_os = "macos")]
    fn first_file_under(root: &std::path::Path) -> Option<std::path::PathBuf> {
        let mut directories = Vec::new();
        for entry in std::fs::read_dir(root).ok()?.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_file() => return Some(path),
                Ok(kind) if kind.is_dir() => directories.push(path),
                _ => {}
            }
        }
        directories.into_iter().find_map(|directory| {
            std::fs::read_dir(directory)
                .ok()?
                .flatten()
                .find_map(|entry| {
                    entry
                        .file_type()
                        .is_ok_and(|kind| kind.is_file())
                        .then(|| entry.path())
                })
        })
    }

    #[test]
    fn command_inventory_is_closed() {
        assert!(allowed_cargo_arguments(&["--version".into()]));
        assert!(allowed_cargo_arguments(&[
            "test".into(),
            "--locked".into(),
            "-p".into(),
            "gate".into(),
            "--all-targets".into(),
        ]));
        assert!(!allowed_cargo_arguments(&["test".into()]));
        assert!(!allowed_cargo_arguments(&["metadata".into()]));
    }
}
