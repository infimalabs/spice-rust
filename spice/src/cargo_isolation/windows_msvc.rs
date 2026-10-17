//! Pinned Windows MSVC C toolchain materialization and target-scoped admission.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read as _;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::IsolationError;

const DECLARATION: &str = "dist/windows-msvc.toml";
const STORE: &str = "spice/windows-msvc-v1";
const RECEIPT: &str = "receipt.json";
const WINDOWS_TARGET: &str = "x86_64-pc-windows-msvc";
const SCHEMA: u64 = 1;
const LAYOUT_VERSION: u64 = 3;
const SHA256_HEX_LENGTH: usize = 64;

#[derive(Clone, Debug, Deserialize)]
struct Declaration {
    schema: u64,
    target: String,
    operator_acceptance: String,
    crt: String,
    sdk: String,
    channel_manifest_sha256: String,
    package_manifest_sha256: String,
    xwin: ToolPin,
    clang_cl: ToolPin,
    llvm_lib: ToolPin,
    llvm_ml: ToolPin,
    lld_link: ToolPin,
}

#[derive(Clone, Debug, Deserialize)]
struct ToolPin {
    version: String,
    path: PathBuf,
    sha256: String,
    bottle_sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Receipt {
    schema: u64,
    generation: String,
    payload_sha256: String,
    channel_manifest_sha256: String,
    package_manifest_sha256: String,
}

/// One completed Windows toolchain publication.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WindowsMsvcReport {
    /// Content-addressed local generation.
    pub generation: String,
    /// Materialized CRT/SDK root.
    pub sysroot: PathBuf,
    /// Whether the exact generation already existed.
    pub reused: bool,
    /// Pinned xwin release.
    pub xwin: String,
    /// Pinned LLVM release.
    pub llvm: String,
}

#[derive(Debug)]
struct Admitted {
    root: PathBuf,
    declaration: Declaration,
}

/// Materialize the declared Windows CRT and SDK after exact operator acceptance.
///
/// The xwin and LLVM executables must already be installed at the pinned paths;
/// this surface verifies their bytes before xwin can fetch Microsoft payloads.
///
/// # Errors
///
/// Refuses an absent/mismatched acceptance, malformed declaration, altered host
/// tool, changed upstream manifest, failed xwin run, or corrupt publication.
pub fn materialize(
    repository: &Path,
    bootstrap_home: &Path,
    acceptance: &str,
) -> Result<WindowsMsvcReport, IsolationError> {
    let (declaration, generation) = read_declaration(repository)?;
    if acceptance != declaration.operator_acceptance {
        return Err(IsolationError::new(
            "cargo-isolation::windows-msvc-license-unaccepted",
            format!(
                "materialization requires recorded operator acceptance {:?}",
                declaration.operator_acceptance
            ),
        ));
    }
    verify_tools(&declaration)?;
    let generations = bootstrap_home.join(STORE).join("generations");
    fs::create_dir_all(&generations).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-publication-failed",
            &generations,
            error,
        )
    })?;
    let destination = generations.join(&generation);
    if destination.exists() {
        let admitted = admit_at(&declaration, &generation, &destination)?;
        return Ok(report(admitted, generation, true));
    }

    let staging = generations.join(format!(".{generation}-{}.tmp", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| {
            IsolationError::io(
                "cargo-isolation::windows-msvc-publication-failed",
                &staging,
                error,
            )
        })?;
    }
    fs::create_dir(&staging).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-publication-failed",
            &staging,
            error,
        )
    })?;
    let result = populate(&declaration, &generation, &staging);
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    match fs::rename(&staging, &destination) {
        Ok(()) => {}
        Err(error) if destination.exists() => {
            fs::remove_dir_all(&staging).map_err(|cleanup| {
                IsolationError::io(
                    "cargo-isolation::windows-msvc-publication-failed",
                    &staging,
                    cleanup,
                )
            })?;
            let _ = error;
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(IsolationError::io(
                "cargo-isolation::windows-msvc-publication-failed",
                &destination,
                error,
            ));
        }
    }
    let admitted = admit_at(&declaration, &generation, &destination)?;
    Ok(report(admitted, generation, false))
}

fn populate(
    declaration: &Declaration,
    generation: &str,
    staging: &Path,
) -> Result<(), IsolationError> {
    let cache = staging.join("cache");
    let payload = staging.join("payload");
    let sysroot = payload.join("sysroot");
    fs::create_dir_all(&payload).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-publication-failed",
            &payload,
            error,
        )
    })?;

    run_xwin(declaration, &cache, None, ["list"])?;
    let channel_manifest = cache.join("dl/manifest_17.json");
    require_hash(
        &channel_manifest,
        &declaration.channel_manifest_sha256,
        "cargo-isolation::windows-msvc-manifest-mismatch",
    )?;
    package_manifest(&cache, &declaration.package_manifest_sha256)?;
    run_xwin(
        declaration,
        &cache,
        Some(&channel_manifest),
        [
            "splat",
            "--use-winsysroot-style",
            "--preserve-ms-arch-notation",
            "--output",
            sysroot.to_str().ok_or_else(|| {
                IsolationError::new(
                    "cargo-isolation::windows-msvc-publication-failed",
                    format!("non-UTF-8 sysroot path {sysroot:?}"),
                )
            })?,
        ],
    )?;
    write_wrappers(declaration, &payload.join("bin"))?;
    fs::remove_dir_all(&cache).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-publication-failed",
            &cache,
            error,
        )
    })?;
    make_immutable(&payload)?;
    let receipt = Receipt {
        schema: SCHEMA,
        generation: generation.to_owned(),
        payload_sha256: tree_digest(&payload)?,
        channel_manifest_sha256: declaration.channel_manifest_sha256.clone(),
        package_manifest_sha256: declaration.package_manifest_sha256.clone(),
    };
    let receipt_path = staging.join(RECEIPT);
    fs::write(
        &receipt_path,
        serde_json::to_vec_pretty(&receipt).expect("receipt serialization cannot fail"),
    )
    .map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-publication-failed",
            &receipt_path,
            error,
        )
    })?;
    fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o444)).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-publication-failed",
            &receipt_path,
            error,
        )
    })?;
    fs::set_permissions(staging, fs::Permissions::from_mode(0o555)).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-publication-failed",
            staging,
            error,
        )
    })?;
    debug_assert_eq!(receipt.generation, generation);
    Ok(())
}

fn run_xwin<'a>(
    declaration: &Declaration,
    cache: &Path,
    manifest: Option<&Path>,
    operation: impl IntoIterator<Item = &'a str>,
) -> Result<(), IsolationError> {
    let mut command = Command::new(&declaration.xwin.path);
    command
        .arg("--accept-license")
        .args(["--arch", "x86_64", "--cache-dir"])
        .arg(cache)
        .args(["--crt-version", &declaration.crt])
        .args(["--sdk-version", &declaration.sdk]);
    if let Some(manifest) = manifest {
        command.arg("--manifest").arg(manifest);
    }
    let output = command.args(operation).output().map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-materialization-failed",
            &declaration.xwin.path,
            error,
        )
    })?;
    require_success("xwin", output)
}

fn require_success(name: &str, output: Output) -> Result<(), IsolationError> {
    if output.status.success() {
        return Ok(());
    }
    Err(IsolationError::new(
        "cargo-isolation::windows-msvc-materialization-failed",
        format!(
            "{name} exited {:?}; stdout={}; stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    ))
}

fn package_manifest(cache: &Path, expected: &str) -> Result<PathBuf, IsolationError> {
    let directory = cache.join("dl");
    let entries = fs::read_dir(&directory).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-manifest-mismatch",
            &directory,
            error,
        )
    })?;
    let mut matched = None;
    for entry in entries {
        let path = entry
            .map_err(|error| {
                IsolationError::io(
                    "cargo-isolation::windows-msvc-manifest-mismatch",
                    &directory,
                    error,
                )
            })?
            .path();
        if path.extension() == Some(OsStr::new("vsman")) && sha256_file(&path)? == expected {
            if matched.replace(path).is_some() {
                return Err(IsolationError::new(
                    "cargo-isolation::windows-msvc-manifest-mismatch",
                    "more than one package manifest has the pinned identity",
                ));
            }
        }
    }
    matched.ok_or_else(|| {
        IsolationError::new(
            "cargo-isolation::windows-msvc-manifest-mismatch",
            format!("downloaded no package manifest with sha256 {expected}"),
        )
    })
}

fn write_wrappers(declaration: &Declaration, directory: &Path) -> Result<(), IsolationError> {
    fs::create_dir(directory).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-publication-failed",
            directory,
            error,
        )
    })?;
    let clang = shell_path(&declaration.clang_cl.path)?;
    let assembler = shell_path(&declaration.llvm_ml.path)?;
    let relative_sysroot = "${0%/*}/../sysroot";
    for (name, body) in [
        (
            "cl.exe",
            format!(
                "#!/bin/sh\nexec '{clang}' --driver-mode=cl --target={WINDOWS_TARGET} /winsysroot \"{relative_sysroot}\" \"$@\"\n"
            ),
        ),
        (
            "ml64.exe",
            format!("#!/bin/sh\nexec '{assembler}' --m64 \"$@\"\n"),
        ),
        ("lib.exe", wrapper(&shell_path(&declaration.llvm_lib.path)?)),
        (
            "link.exe",
            wrapper(&shell_path(&declaration.lld_link.path)?),
        ),
    ] {
        let path = directory.join(name);
        fs::write(&path, body).map_err(|error| {
            IsolationError::io(
                "cargo-isolation::windows-msvc-publication-failed",
                &path,
                error,
            )
        })?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).map_err(|error| {
            IsolationError::io(
                "cargo-isolation::windows-msvc-publication-failed",
                &path,
                error,
            )
        })?;
    }
    Ok(())
}

fn wrapper(tool: &str) -> String {
    format!("#!/bin/sh\nexec '{tool}' \"$@\"\n")
}

fn shell_path(path: &Path) -> Result<String, IsolationError> {
    let text = path.to_str().filter(|text| !text.contains('\''));
    text.map(str::to_owned).ok_or_else(|| {
        IsolationError::new(
            "cargo-isolation::windows-msvc-tool-invalid",
            format!("toolchain path cannot be represented safely in a wrapper: {path:?}"),
        )
    })
}

/// Bind the verified toolchain only when Cargo selects the Windows MSVC target.
///
/// # Errors
///
/// Refuses a missing or altered declared generation or host LLVM component.
pub fn bind_if_selected(
    repository: &Path,
    bootstrap_home: &Path,
    arguments: &[OsString],
    command: &mut Command,
) -> Result<(), IsolationError> {
    if !selects_windows_target(arguments, std::env::var_os("CARGO_BUILD_TARGET").as_deref()) {
        return Ok(());
    }
    let (declaration, generation) = read_declaration(repository)?;
    verify_tools(&declaration)?;
    let root = bootstrap_home
        .join(STORE)
        .join("generations")
        .join(&generation);
    let admitted = admit_at(&declaration, &generation, &root)?;
    let bin = admitted.root.join("payload/bin");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut bound_path = bin.as_os_str().to_owned();
    if !path.is_empty() {
        bound_path.push(OsStr::new(":"));
        bound_path.push(path);
    }
    command
        .env("PATH", bound_path)
        .env("CC_x86_64_pc_windows_msvc", "cl.exe")
        .env("CXX_x86_64_pc_windows_msvc", "cl.exe")
        .env("AR_x86_64_pc_windows_msvc", "lib.exe")
        .env(
            "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER",
            bin.join("link.exe"),
        );
    Ok(())
}

fn selects_windows_target(arguments: &[OsString], ambient: Option<&OsStr>) -> bool {
    let windows = OsStr::new(WINDOWS_TARGET);
    let mut flagged = false;
    let mut selects_windows = false;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let target = if argument == OsStr::new("--target") {
            arguments.next().map(OsString::as_os_str)
        } else if let Some(target) = argument
            .to_str()
            .and_then(|argument| argument.strip_prefix("--target="))
        {
            Some(OsStr::new(target))
        } else {
            continue;
        };
        flagged = true;
        selects_windows |= target == Some(windows);
    }
    if flagged {
        selects_windows
    } else {
        ambient == Some(windows)
    }
}

fn admit_at(
    declaration: &Declaration,
    generation: &str,
    root: &Path,
) -> Result<Admitted, IsolationError> {
    let receipt_path = root.join(RECEIPT);
    let bytes = fs::read(&receipt_path).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-toolchain-unavailable",
            &receipt_path,
            error,
        )
    })?;
    let receipt: Receipt = serde_json::from_slice(&bytes).map_err(|error| {
        IsolationError::new(
            "cargo-isolation::windows-msvc-toolchain-corrupt",
            format!("{} is invalid: {error}", receipt_path.display()),
        )
    })?;
    if receipt.schema != SCHEMA
        || receipt.generation != generation
        || receipt.channel_manifest_sha256 != declaration.channel_manifest_sha256
        || receipt.package_manifest_sha256 != declaration.package_manifest_sha256
    {
        return Err(IsolationError::new(
            "cargo-isolation::windows-msvc-toolchain-corrupt",
            format!("{} disagrees with {DECLARATION}", receipt_path.display()),
        ));
    }
    let observed = tree_digest(&root.join("payload"))?;
    if observed != receipt.payload_sha256 {
        return Err(IsolationError::new(
            "cargo-isolation::windows-msvc-toolchain-corrupt",
            format!(
                "{} expected payload {}, observed {observed}",
                root.display(),
                receipt.payload_sha256
            ),
        ));
    }
    Ok(Admitted {
        root: root.to_path_buf(),
        declaration: declaration.clone(),
    })
}

fn read_declaration(repository: &Path) -> Result<(Declaration, String), IsolationError> {
    let path = repository.join(DECLARATION);
    let bytes = fs::read(&path).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-declaration-unavailable",
            &path,
            error,
        )
    })?;
    let declaration: Declaration = toml::from_slice(&bytes).map_err(|error| {
        IsolationError::new(
            "cargo-isolation::windows-msvc-declaration-invalid",
            format!("{}: {error}", path.display()),
        )
    })?;
    if declaration.schema != SCHEMA || declaration.target != WINDOWS_TARGET {
        return Err(IsolationError::new(
            "cargo-isolation::windows-msvc-declaration-invalid",
            format!(
                "{} must declare schema {SCHEMA} and target {WINDOWS_TARGET}",
                path.display()
            ),
        ));
    }
    for digest in [
        &declaration.channel_manifest_sha256,
        &declaration.package_manifest_sha256,
        &declaration.xwin.sha256,
        &declaration.clang_cl.sha256,
        &declaration.llvm_lib.sha256,
        &declaration.llvm_ml.sha256,
        &declaration.lld_link.sha256,
    ] {
        require_digest(digest)?;
    }
    let generation = generation_digest(&bytes);
    Ok((declaration, generation))
}

fn verify_tools(declaration: &Declaration) -> Result<(), IsolationError> {
    for (name, pin) in [
        ("xwin", &declaration.xwin),
        ("clang-cl", &declaration.clang_cl),
        ("llvm-lib", &declaration.llvm_lib),
        ("llvm-ml", &declaration.llvm_ml),
        ("lld-link", &declaration.lld_link),
    ] {
        require_digest(&pin.bottle_sha256)?;
        require_hash(
            &pin.path,
            &pin.sha256,
            "cargo-isolation::windows-msvc-tool-altered",
        )
        .map_err(|error| {
            IsolationError::new(
                error.reason(),
                format!(
                    "{name} {} (bottle {}): {}",
                    pin.version,
                    pin.bottle_sha256,
                    error.detail()
                ),
            )
        })?;
    }
    Ok(())
}

fn report(admitted: Admitted, generation: String, reused: bool) -> WindowsMsvcReport {
    WindowsMsvcReport {
        generation,
        sysroot: admitted.root.join("payload/sysroot"),
        reused,
        xwin: admitted.declaration.xwin.version,
        llvm: admitted.declaration.clang_cl.version,
    }
}

fn require_hash(path: &Path, expected: &str, reason: &'static str) -> Result<(), IsolationError> {
    let observed =
        sha256_file(path).map_err(|error| IsolationError::new(reason, error.detail()))?;
    if observed == expected {
        Ok(())
    } else {
        Err(IsolationError::new(
            reason,
            format!(
                "{} expected sha256 {expected}, observed {observed}",
                path.display()
            ),
        ))
    }
}

fn sha256_file(path: &Path) -> Result<String, IsolationError> {
    let mut file = fs::File::open(path).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-toolchain-unavailable",
            path,
            error,
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            IsolationError::io(
                "cargo-isolation::windows-msvc-toolchain-unavailable",
                path,
                error,
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn generation_digest(declaration: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"spice-windows-msvc-generation-v1\0");
    hasher.update(LAYOUT_VERSION.to_le_bytes());
    hasher.update(declaration);
    format!("{:x}", hasher.finalize())
}

fn require_digest(digest: &str) -> Result<(), IsolationError> {
    if digest.len() == SHA256_HEX_LENGTH && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(IsolationError::new(
            "cargo-isolation::windows-msvc-declaration-invalid",
            format!("invalid sha256 identity {digest:?}"),
        ))
    }
}

fn tree_digest(root: &Path) -> Result<String, IsolationError> {
    let mut paths = Vec::new();
    collect(root, &mut paths)?;
    paths.sort();
    let mut hasher = Sha256::new();
    hasher.update(b"spice-windows-msvc-payload-v1\0");
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .expect("collected path is beneath root");
        hasher.update(relative.as_os_str().as_bytes());
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            IsolationError::io(
                "cargo-isolation::windows-msvc-toolchain-unavailable",
                &path,
                error,
            )
        })?;
        if metadata.file_type().is_symlink() {
            hasher.update(b"\0link\0");
            let target = fs::read_link(&path).map_err(|error| {
                IsolationError::io(
                    "cargo-isolation::windows-msvc-toolchain-unavailable",
                    &path,
                    error,
                )
            })?;
            hasher.update(target.as_os_str().as_bytes());
        } else if metadata.is_file() {
            hasher.update(b"\0file\0");
            hasher.update(metadata.permissions().mode().to_le_bytes());
            hasher.update(sha256_file(&path)?.as_bytes());
        } else {
            return Err(IsolationError::new(
                "cargo-isolation::windows-msvc-toolchain-corrupt",
                format!("unsupported payload entry {path:?}"),
            ));
        }
        hasher.update(b"\0");
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), IsolationError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-toolchain-unavailable",
            directory,
            error,
        )
    })?;
    for entry in entries {
        let path = entry
            .map_err(|error| {
                IsolationError::io(
                    "cargo-isolation::windows-msvc-toolchain-unavailable",
                    directory,
                    error,
                )
            })?
            .path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            IsolationError::io(
                "cargo-isolation::windows-msvc-toolchain-unavailable",
                &path,
                error,
            )
        })?;
        if metadata.is_dir() {
            collect(&path, paths)?;
        } else {
            paths.push(path);
        }
    }
    Ok(())
}

fn make_immutable(path: &Path) -> Result<(), IsolationError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-publication-failed",
            path,
            error,
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| {
            IsolationError::io(
                "cargo-isolation::windows-msvc-publication-failed",
                path,
                error,
            )
        })? {
            make_immutable(
                &entry
                    .map_err(|error| {
                        IsolationError::io(
                            "cargo-isolation::windows-msvc-publication-failed",
                            path,
                            error,
                        )
                    })?
                    .path(),
            )?;
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o555))
    } else {
        let mode = if metadata.permissions().mode() & 0o111 == 0 {
            0o444
        } else {
            0o555
        };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    .map_err(|error| {
        IsolationError::io(
            "cargo-isolation::windows-msvc-publication-failed",
            path,
            error,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_target_selection_is_exact_and_target_scoped() {
        assert!(selects_windows_target(
            &["check".into(), "--target".into(), WINDOWS_TARGET.into()],
            None,
        ));
        assert!(selects_windows_target(
            &[format!("--target={WINDOWS_TARGET}").into()],
            None,
        ));
        assert!(selects_windows_target(
            &[],
            Some(OsStr::new(WINDOWS_TARGET))
        ));
        assert!(!selects_windows_target(
            &["check".into(), "--target=wasm32-unknown-unknown".into()],
            None,
        ));
        assert!(!selects_windows_target(
            &["check".into(), "--target=wasm32-unknown-unknown".into()],
            Some(OsStr::new(WINDOWS_TARGET)),
        ));
        assert!(selects_windows_target(
            &[
                "check".into(),
                "--target".into(),
                WINDOWS_TARGET.into(),
                "--target=aarch64-apple-darwin".into(),
            ],
            None,
        ));
        assert!(selects_windows_target(
            &[
                "--target=aarch64-apple-darwin".into(),
                "--target".into(),
                WINDOWS_TARGET.into(),
            ],
            None,
        ));
    }

    #[test]
    fn payload_identity_changes_with_file_bytes_and_wrapper_mode() {
        let root = std::env::temp_dir().join(format!(
            "spice-windows-msvc-digest-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).unwrap();
        let file = root.join("header.h");
        fs::write(&file, "first").unwrap();
        let first = tree_digest(&root).unwrap();
        fs::write(&file, "other").unwrap();
        assert_ne!(tree_digest(&root).unwrap(), first);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        let executable = tree_digest(&root).unwrap();
        assert_ne!(executable, first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_refuses_an_altered_materialized_component() {
        let root = std::env::temp_dir().join(format!(
            "spice-windows-msvc-admission-{}",
            uuid::Uuid::new_v4()
        ));
        let payload = root.join("payload");
        fs::create_dir_all(&payload).unwrap();
        let header = payload.join("header.h");
        fs::write(&header, "first").unwrap();

        let digest = "a".repeat(64);
        let tool = ToolPin {
            version: "test".into(),
            path: PathBuf::from("/test/tool"),
            sha256: digest.clone(),
            bottle_sha256: digest.clone(),
        };
        let declaration = Declaration {
            schema: SCHEMA,
            target: WINDOWS_TARGET.into(),
            operator_acceptance: "test".into(),
            crt: "test".into(),
            sdk: "test".into(),
            channel_manifest_sha256: digest.clone(),
            package_manifest_sha256: digest,
            xwin: tool.clone(),
            clang_cl: tool.clone(),
            llvm_lib: tool.clone(),
            llvm_ml: tool.clone(),
            lld_link: tool,
        };
        let generation = "generation";
        let receipt = Receipt {
            schema: SCHEMA,
            generation: generation.into(),
            payload_sha256: tree_digest(&payload).unwrap(),
            channel_manifest_sha256: declaration.channel_manifest_sha256.clone(),
            package_manifest_sha256: declaration.package_manifest_sha256.clone(),
        };
        fs::write(root.join(RECEIPT), serde_json::to_vec(&receipt).unwrap()).unwrap();
        admit_at(&declaration, generation, &root).unwrap();

        fs::write(&header, "other").unwrap();
        let error = admit_at(&declaration, generation, &root).unwrap_err();
        assert_eq!(
            error.reason(),
            "cargo-isolation::windows-msvc-toolchain-corrupt"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
