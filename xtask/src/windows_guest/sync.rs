//! Transfer of admitted x64 artifacts and tracked source, and argv execution.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::{Read as _, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use windows_guest::{
    CommandResult, Completion, EXECUTOR_PATH, Limits, MAX_OUTPUT_BYTES, MAX_RUNTIME_SECONDS,
    PROTOCOL_VERSION, Request,
};

use super::declaration::Machine;
use super::machine::{HostKey, Paths, remove_tree, running, ssh};
use super::{WindowsGuestError, run_tool, usage};

const TAR: &str = "/usr/bin/tar";
const GIT: &str = "git";
const CROSS_TARGET: &str = "target/x86_64-pc-windows-msvc";
const EXECUTOR_ARTIFACT: &str = "target/x86_64-pc-windows-msvc/release/windows-guest.exe";
const BUILD_EXECUTOR: &str =
    "spice dev cargo -- build --locked --release --target x86_64-pc-windows-msvc -p windows-guest";
/// Replace the synced tree, then unpack the transferred archive beneath it.
const RECEIVE: &str = r"(if exist C:\spice\work rmdir /s /q C:\spice\work) & C:\Windows\System32\tar.exe -xf - -C C:\spice";
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const PE_HEADER_PREFIX: usize = 4096;
/// Exit status the guest executor uses when it refuses a request.
const EXECUTOR_REFUSAL: i32 = 70;

/// Paths named for one sync.
pub(super) struct Selection {
    artifacts: Vec<PathBuf>,
    sources: Vec<PathBuf>,
}

impl Selection {
    pub(super) fn parse(arguments: Vec<OsString>) -> Result<Self, WindowsGuestError> {
        let mut selection = Self {
            artifacts: Vec::new(),
            sources: Vec::new(),
        };
        let mut arguments = arguments.into_iter();
        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| usage("sync options each take one path"))?;
            match flag.to_str() {
                Some("--artifact") => selection.artifacts.push(value.into()),
                Some("--source") => selection.sources.push(value.into()),
                _ => return Err(usage("sync accepts only --artifact PATH and --source PATH")),
            }
        }
        Ok(selection)
    }
}

/// One argv and its bounds.
pub(super) struct RunArguments {
    limits: Limits,
    program: String,
    arguments: Vec<String>,
}

impl RunArguments {
    pub(super) fn parse(arguments: Vec<OsString>) -> Result<Self, WindowsGuestError> {
        let mut limits = Limits {
            runtime_seconds: MAX_RUNTIME_SECONDS,
            output_bytes: MAX_OUTPUT_BYTES,
        };
        let mut arguments = arguments.into_iter();
        loop {
            let option = arguments
                .next()
                .ok_or_else(|| usage("run requires -- PROGRAM [ARG...]"))?;
            match option.to_str() {
                Some("--") => break,
                Some("--timeout") => limits.runtime_seconds = number(arguments.next())?,
                Some("--output-bytes") => limits.output_bytes = number(arguments.next())?,
                _ => return Err(usage("run accepts --timeout, --output-bytes, then --")),
            }
        }
        let mut argv = arguments.map(|argument| {
            argument
                .into_string()
                .map_err(|_| usage("guest argv must be Unicode"))
        });
        let program = argv
            .next()
            .ok_or_else(|| usage("run requires a PROGRAM after --"))??;
        Ok(Self {
            limits,
            program,
            arguments: argv.collect::<Result<_, _>>()?,
        })
    }
}

fn number(value: Option<OsString>) -> Result<u32, WindowsGuestError> {
    value
        .as_deref()
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| usage("--timeout and --output-bytes take a whole number"))
}

/// Replace the guest's synced tree with the executor, the named x64 artifacts
/// under `bin`, and the named tracked source under `source`.
pub(super) fn sync(
    paths: &Paths,
    machine: &Machine,
    root: &Path,
    selection: &Selection,
    output: &mut impl Write,
) -> Result<(), String> {
    require_running(machine)?;
    let cross = root.join(CROSS_TARGET);
    let stage = paths.stage();
    remove_tree(&stage)?;
    let bin = stage.join("work").join("bin");
    let source = stage.join("work").join("source");
    let runner = stage.join("runner");
    for directory in [&bin, &source, &runner] {
        fs::create_dir_all(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    }
    let executor = root.join(EXECUTOR_ARTIFACT);
    if !executor.is_file() {
        return Err(format!(
            "the guest executor {EXECUTOR_ARTIFACT} is missing; build it with `{BUILD_EXECUTOR}`"
        ));
    }
    admit_artifact(&cross, &executor)?;
    copy(&executor, &runner.join("windows-guest.exe"))?;

    let mut names = BTreeSet::new();
    for artifact in &selection.artifacts {
        let artifact = admit_artifact(&cross, artifact)?;
        let name = artifact
            .file_name()
            .ok_or_else(|| format!("artifact {} has no file name", artifact.display()))?;
        if !names.insert(name.to_owned()) {
            return Err(format!(
                "two artifacts share the file name {}",
                name.to_string_lossy()
            ));
        }
        copy(&artifact, &bin.join(name))?;
    }
    let mut source_files = 0;
    for selected in &selection.sources {
        for relative in tracked_files(root, selected)? {
            let destination = source.join(&relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
            copy(&root.join(&relative), &destination)?;
            source_files += 1;
        }
    }
    transfer(paths, machine, &stage)?;
    remove_tree(&stage)?;
    writeln!(
        output,
        "synced the guest executor, {} artifact(s), and {source_files} tracked source file(s) to C:\\spice\\work",
        names.len()
    )
    .map_err(|error| error.to_string())
}

/// Run one argv in the guest, write its bounded output, and return its exact
/// exit status.
pub(super) fn run(
    paths: &Paths,
    machine: &Machine,
    run: RunArguments,
    output: &mut impl Write,
    error_output: &mut impl Write,
) -> Result<i32, String> {
    let request = Request {
        version: PROTOCOL_VERSION,
        limits: run.limits,
        command: windows_guest::Command {
            program: run.program,
            arguments: run.arguments,
        },
    };
    request.admit().map_err(|error| error.to_string())?;
    require_running(machine)?;
    let body =
        serde_json::to_vec(&request).map_err(|error| format!("cannot encode request: {error}"))?;
    let mut child = ssh(paths, machine, HostKey::Pinned)
        .arg(EXECUTOR_PATH)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start ssh: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "ssh stdin is unavailable".to_owned())?;
    stdin
        .write_all(&body)
        .map_err(|error| format!("cannot send request: {error}"))?;
    drop(stdin);
    let transport = child
        .wait_with_output()
        .map_err(|error| format!("cannot wait for ssh: {error}"))?;
    if !transport.status.success() {
        let stderr = String::from_utf8_lossy(&transport.stderr);
        return Err(if transport.status.code() == Some(EXECUTOR_REFUSAL) {
            // The executor's own stderr already names the refusal.
            stderr.trim().to_owned()
        } else {
            format!(
                "guest transport failed with {}: {}; run windows-guest sync if the executor is missing",
                transport.status,
                stderr.trim()
            )
        });
    }
    let result: CommandResult = serde_json::from_slice(&transport.stdout)
        .map_err(|error| format!("guest result is malformed: {error}"))?;
    let stdout = result
        .stdout
        .bytes()
        .map_err(|error| format!("guest stdout is malformed: {error}"))?;
    let stderr = result
        .stderr
        .bytes()
        .map_err(|error| format!("guest stderr is malformed: {error}"))?;
    output
        .write_all(&stdout)
        .and_then(|()| output.flush())
        .map_err(|error| format!("cannot write output: {error}"))?;
    error_output
        .write_all(&stderr)
        .map_err(|error| format!("cannot write output: {error}"))?;
    let mut report = match result.completion {
        Completion::Exited { code } => format!("[windows-guest] exit code {code} ({code:#010x})"),
        Completion::TimedOut => format!(
            "[windows-guest] timed out after {}s; process tree terminated",
            request.limits.runtime_seconds
        ),
    };
    for (name, capture) in [("stdout", &result.stdout), ("stderr", &result.stderr)] {
        if capture.truncated {
            report.push_str(&format!("; {name} truncated"));
        }
    }
    writeln!(error_output, "{report}").map_err(|error| format!("cannot write output: {error}"))?;
    Ok(result.completion.exit_code())
}

fn require_running(machine: &Machine) -> Result<(), String> {
    if running(machine.qmp_port) {
        Ok(())
    } else {
        Err("the guest is not running; run windows-guest start".to_owned())
    }
}

/// Admit only an x86-64 PE file produced beneath the pinned cross target.
fn admit_artifact(cross: &Path, artifact: &Path) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(artifact)
        .map_err(|error| format!("cannot resolve artifact {}: {error}", artifact.display()))?;
    let cross = fs::canonicalize(cross).map_err(|error| {
        format!(
            "the pinned cross target {} is unavailable: {error}",
            cross.display()
        )
    })?;
    if !canonical.starts_with(&cross) || !canonical.is_file() {
        return Err(format!(
            "artifact {} is not a file beneath {CROSS_TARGET}",
            artifact.display()
        ));
    }
    let mut header = Vec::with_capacity(PE_HEADER_PREFIX);
    fs::File::open(&canonical)
        .and_then(|file| file.take(PE_HEADER_PREFIX as u64).read_to_end(&mut header))
        .map_err(|error| format!("cannot read {}: {error}", canonical.display()))?;
    if is_x86_64_pe(&header) {
        Ok(canonical)
    } else {
        Err(format!(
            "artifact {} is not an x86-64 Windows PE image",
            artifact.display()
        ))
    }
}

/// Whether a file prefix is a PE image whose COFF machine is x86-64.
fn is_x86_64_pe(header: &[u8]) -> bool {
    if header.get(..2) != Some(b"MZ") {
        return false;
    }
    let Some(offset) = header
        .get(0x3c..0x40)
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
    else {
        return false;
    };
    let Some(signature) = offset
        .checked_add(6)
        .and_then(|end| header.get(offset..end))
    else {
        return false;
    };
    signature[..4] == *b"PE\0\0"
        && u16::from_le_bytes([signature[4], signature[5]]) == IMAGE_FILE_MACHINE_AMD64
}

/// Repository-relative tracked files beneath one selected source path.
fn tracked_files(root: &Path, selected: &Path) -> Result<Vec<PathBuf>, String> {
    if selected.is_absolute() {
        return Err(format!(
            "source {} must be relative to the worktree root",
            selected.display()
        ));
    }
    let listed = run_tool(
        Command::new(GIT)
            .current_dir(root)
            .args(["ls-files", "-z", "--full-name", "--"])
            .arg(selected),
        "git ls-files",
    )?;
    let files = listed
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Err(format!(
            "source {} names no tracked file",
            selected.display()
        ));
    }
    Ok(files)
}

fn transfer(paths: &Paths, machine: &Machine, stage: &Path) -> Result<(), String> {
    let mut archive = Command::new(TAR)
        .env("COPYFILE_DISABLE", "1")
        .args(["--no-xattrs", "--no-mac-metadata", "-cf", "-", "-C"])
        .arg(stage)
        .args(["runner", "work"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start tar: {error}"))?;
    let stream = archive
        .stdout
        .take()
        .ok_or_else(|| "tar output is unavailable".to_owned())?;
    let received = ssh(paths, machine, HostKey::Pinned)
        .arg(RECEIVE)
        .stdin(Stdio::from(stream))
        .stdout(Stdio::null())
        .output()
        .map_err(|error| format!("cannot start ssh: {error}"));
    let archived = archive
        .wait()
        .map_err(|error| format!("cannot wait for tar: {error}"))?;
    let received = received?;
    if !archived.success() {
        return Err(format!("tar failed with {archived}"));
    }
    if !received.status.success() {
        return Err(format!(
            "guest refused the transfer with {}: {}",
            received.status,
            String::from_utf8_lossy(&received.stderr).trim()
        ));
    }
    Ok(())
}

fn copy(source: &Path, destination: &Path) -> Result<(), String> {
    fs::copy(source, destination).map(|_| ()).map_err(|error| {
        format!(
            "cannot copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pe(machine: u16, offset: u32) -> Vec<u8> {
        let mut header = vec![0_u8; 0x200];
        header[..2].copy_from_slice(b"MZ");
        header[0x3c..0x40].copy_from_slice(&offset.to_le_bytes());
        let offset = offset as usize;
        header[offset..offset + 4].copy_from_slice(b"PE\0\0");
        header[offset + 4..offset + 6].copy_from_slice(&machine.to_le_bytes());
        header
    }

    #[test]
    fn only_x86_64_pe_images_are_admitted() {
        assert!(is_x86_64_pe(&pe(IMAGE_FILE_MACHINE_AMD64, 0x80)));
        // A native ARM64 image would bypass the pinned x64 cross-build.
        assert!(!is_x86_64_pe(&pe(0xaa64, 0x80)));
        assert!(!is_x86_64_pe(&pe(0x014c, 0x80)));
        let mut not_dos = pe(IMAGE_FILE_MACHINE_AMD64, 0x80);
        not_dos[0] = 0xcf;
        assert!(!is_x86_64_pe(&not_dos));
        let mut beyond_prefix = pe(IMAGE_FILE_MACHINE_AMD64, 0x80);
        beyond_prefix[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!is_x86_64_pe(&beyond_prefix));
    }
}
