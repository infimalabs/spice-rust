//! Repository-owned Darwin implementation of the native proof-runner protocol.

#![forbid(unsafe_code)]

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};

use proof::challenge::{Challenge, ChallengeError};
use serde::Deserialize;
use wire::{Digest, Domain, IdentError};

const PROTOCOL: &str = "spice-proof-native-runner-v1";
const ATTESTATION: &str = "spice-proof-native-attestation-v1";
const ISOLATION: &str = concat!(
    "repository=read-only:",
    "cargo-home=fresh-empty:",
    "cargo-target=fresh-empty:",
    "network=denied:",
    "host-dependencies=denied"
);
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

    let temporary = Temporary::fresh(&challenge)?;
    let profile = sandbox_profile(&workspace, &pins, &temporary)?;
    temporary.write_cargo_config(&workspace)?;

    eprintln!("{ATTESTATION}:runner={identity}:challenge={challenge}:{ISOLATION}");
    io::stderr()
        .flush()
        .map_err(|error| format!("cannot flush runner attestation: {error}"))?;

    let execution = run_sandboxed(arguments, &workspace, &pins, &temporary, &profile);
    let cleanup = temporary.remove();
    match (execution, cleanup) {
        (Ok(status), Ok(())) => Ok(status.code().unwrap_or(1)),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(execution), Err(cleanup)) => Err(format!(
            "{execution}; native proof cleanup also failed: {cleanup}"
        )),
    }
}

fn allowed_cargo_arguments(arguments: &[OsString]) -> bool {
    const ALLOWED: [&[&str]; 6] = [
        &["--version"],
        &["test", "--locked", "-p", "gate", "--all-targets"],
        &["mutants", "--workspace"],
        &["test", "--locked", "-p", "listen", "--test", "corpus"],
        &["test", "--locked", "-p", "sill", "--lib"],
        &["test", "--locked", "-p", "dist", "--test", "pipeline"],
    ];
    ALLOWED.iter().any(|expected| {
        arguments.len() == expected.len()
            && arguments
                .iter()
                .zip(*expected)
                .all(|(observed, expected)| observed == OsStr::new(expected))
    })
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
        for (label, path) in [
            ("SDK", &self.sdk_path),
            ("developer directory", &self.developer_dir),
        ] {
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tool {
    path: PathBuf,
    digest: Digest,
    #[serde(default)]
    version: Option<String>,
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
        let Some(expected) = &self.version else {
            return Ok(());
        };
        let output = Command::new(&self.path)
            .arg("--version")
            .env_clear()
            .output()
            .map_err(|error| format!("cannot run pinned {name}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "pinned {name} version command failed with {}",
                output.status
            ));
        }
        let observed = one_line(&output.stdout, name)?;
        if &observed != expected {
            return Err(format!(
                "pinned {name} version changed: expected {expected:?}, observed {observed:?}"
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
            temporary: root.join("tmp"),
            root,
        };
        for path in [
            &instance.cargo_home,
            &instance.cargo_target,
            &instance.home,
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

/// The Cargo caches the invoking environment owns, which the sealed build is
/// denied so a warm host cache cannot stand in for the vendored closure.
///
/// This is deliberately observed rather than pinned: the caches worth denying
/// are the ones the ambient environment would have used, named exactly the way
/// Cargo itself names them.
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
        .ok_or_else(|| "the invoking environment names no Cargo home".to_owned())
}

fn sandbox_profile(workspace: &Path, pins: &Pins, temporary: &Temporary) -> Result<String, String> {
    let git_common = git_one_line(
        &pins.git.path,
        workspace,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let git_common = fs::canonicalize(git_common)
        .map_err(|error| format!("cannot resolve Git common directory: {error}"))?;
    let ambient = ambient_cargo_home()?;
    let ambient_registry = ambient.join("registry");
    let ambient_git = ambient.join("git");
    Ok(format!(
        concat!(
            "(version 1)\n",
            "(allow default)\n",
            "(deny network*)\n",
            "(deny file-write* (subpath \"{}\") (subpath \"{}\"))\n",
            "(deny file-read* (subpath \"{}\") (subpath \"{}\"))\n",
            "(allow file-write* (subpath \"{}\"))\n"
        ),
        sandbox_string(workspace)?,
        sandbox_string(&git_common)?,
        sandbox_string(&ambient_registry)?,
        sandbox_string(&ambient_git)?,
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
    use super::allowed_cargo_arguments;

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
