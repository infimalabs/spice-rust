#[cfg(not(target_arch = "wasm32"))]
use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use sill::Deadlines;

const USAGE_EXIT: u8 = 2;
type CommandResult = Result<(), Box<dyn std::error::Error>>;
const USAGE: &str = concat!(
    "usage: cargo xtask <help|verify [--package PACKAGE]|verify-action <selection|battery>|",
    "gate ARM|app|activate-hooks|",
    "install [--host HOST] [--port PORT] [--release] [--no-resident]|",
    "build-cache [--dry-run|--apply] [--max-bytes BYTES] ",
    "[--free-space-floor BYTES] [--retain-live-worktrees N]|",
    "proof-image <build [ARCHIVE]|load ARCHIVE|verify [ARCHIVE]>|",
    "stage-target TARGET|",
    "commit-msg MESSAGE_FILE|reference-transaction STATE>"
);

fn help() -> String {
    let arms = gate::GateId::ALL.map(gate::GateId::as_str).join("\n  ");
    format!(
        concat!(
            "{usage}\n\n",
            "verify-action selection is the ordinary staged-change gate; ",
            "verify-action battery is its explicit complete policy surface. ",
            "Neither is a completion verifier.\n\n",
            "gate ARM accepts:\n  {arms}"
        ),
        usage = USAGE,
        arms = arms,
    )
}

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let command = arguments.next();

    let mut output = io::stdout().lock();
    // Dispatch only current spellings; removed command names have no alias arm.
    let result: CommandResult = match command.as_deref() {
        Some(value) if value == OsStr::new("verify-action") && arguments.len() == 1 => {
            let action = arguments.next().expect("one verifier action was admitted");
            drop(output);
            return run_verify_action(&action, &mut io::stderr().lock());
        }
        Some(value) if value == OsStr::new("help") && arguments.len() == 0 => {
            writeln!(output, "{}", help()).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("verify") => {
            run_verify(arguments.collect(), &mut output)
        }
        Some(value) if value == OsStr::new("__verify-system") && arguments.len() == 0 => {
            xtask::verify_child(&mut output).map_err(Into::into)
        }
        #[cfg(not(target_arch = "wasm32"))]
        Some(value) if is_workspace_test_command(value) => {
            run_workspace_test_command(value, arguments.collect(), &mut output)
        }
        Some(value) if value == OsStr::new("gate") && arguments.len() == 1 => {
            run_gate(arguments.next(), &mut output, &mut io::stderr().lock())
        }
        Some(value) if value == OsStr::new("app") => run_app(arguments.collect(), &mut output),
        Some(value) if value == OsStr::new("activate-hooks") && arguments.len() == 0 => {
            run_activate_hooks(&mut output)
        }
        Some(value) if value == OsStr::new("build-cache") => {
            run_build_cache(arguments.collect(), &mut output)
        }
        Some(value) if value == OsStr::new("install") => {
            drop(output);
            return run_install(arguments.collect());
        }
        Some(value) if value == OsStr::new("proof-image") => {
            drop(output);
            return run_proof_image(arguments.collect());
        }
        Some(value) if value == OsStr::new("stage-target") && arguments.len() == 1 => {
            run_stage_target(arguments.next(), &mut output)
        }
        Some(value) if value == OsStr::new("commit-msg") && arguments.len() == 1 => {
            run_commit_message_command(arguments.next())
        }
        Some(value) if value == OsStr::new("reference-transaction") && arguments.len() == 1 => {
            let state = arguments
                .next()
                .expect("one reference-transaction state was admitted");
            run_reference_transaction(&state)
        }
        _ => {
            eprintln!("{}", help());
            return ExitCode::from(USAGE_EXIT);
        }
    };
    finish(result)
}

fn finish(result: Result<(), Box<dyn std::error::Error>>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.is::<UsageError>() => ExitCode::from(USAGE_EXIT),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run_verify(arguments: Vec<OsString>, output: &mut impl io::Write) -> CommandResult {
    match arguments.as_slice() {
        [] => xtask::verify(output).map_err(Into::into),
        [option, package] if option == OsStr::new("--package") => {
            let Some(package) = package.to_str() else {
                eprintln!("{}", help());
                return Err(usage_error().into());
            };
            xtask::verify_package(package, output).map_err(Into::into)
        }
        _ => {
            eprintln!("{}", help());
            Err(usage_error().into())
        }
    }
}

fn run_verify_action(action: &OsStr, output: &mut impl io::Write) -> ExitCode {
    let result = if action == OsStr::new("selection") {
        xtask::verify_selection_child(output)
    } else if action == OsStr::new("battery") {
        xtask::verify_battery_child(output)
    } else {
        eprintln!("{}", help());
        return ExitCode::from(USAGE_EXIT);
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(xtask::VERIFY_ACTION_REFUSAL_EXIT),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run_app(arguments: Vec<OsString>, output: &mut impl io::Write) -> CommandResult {
    match arguments.as_slice() {
        [] => xtask::app(&release_environment(), output).map_err(Into::into),
        [flag, publish_root] if flag == OsStr::new(gate::VERIFIER_APP_PUBLISH_FLAG) => {
            xtask::app_to(&release_environment(), Path::new(publish_root), output)
                .map_err(Into::into)
        }
        _ => {
            eprintln!("{}", help());
            Err(usage_error().into())
        }
    }
}

fn run_gate(
    argument: Option<OsString>,
    output: &mut impl io::Write,
    progress: &mut impl io::Write,
) -> CommandResult {
    let arm = argument.expect("one gate arm was admitted");
    let Some(arm) = arm.to_str() else {
        eprintln!("{}", help());
        return Err(usage_error().into());
    };
    let Some(gate_id) = gate::GateId::ALL
        .into_iter()
        .find(|gate_id| gate_id.as_str() == arm)
    else {
        eprintln!("unknown gate arm {arm:?}; run `cargo xtask help` for accepted gate arms");
        return Err(usage_error().into());
    };
    if gate::GateId::TEST_EVIDENCE.contains(&gate_id) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let report = xtask::run_test_evidence(ambient_environment()?, progress, output)?;
            return if report
                .instrument_admits(gate_id)
                .expect("a registered test-evidence ID has one report decision")
            {
                Ok(())
            } else {
                Err(xtask::GateArmError::Refused(gate_id).into())
            };
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (output, progress);
            return Err(xtask::GateArmError::Unavailable.into());
        }
    }
    xtask::gate_arm(gate_id, output).map_err(Into::into)
}

fn run_activate_hooks(output: &mut impl io::Write) -> CommandResult {
    // The executable published for the hooks to exec is this one. Cargo keeps
    // it under a content-keyed directory, so this is the only moment its path
    // is known to anything that could copy it.
    env::current_exe()
        .map_err(Into::into)
        .and_then(|entry_point| {
            xtask::activate_hooks(Path::new("."), &entry_point, output).map_err(Into::into)
        })
}

fn run_build_cache(arguments: Vec<OsString>, output: &mut impl io::Write) -> CommandResult {
    let Some(request) = build_cache_request(&arguments) else {
        eprintln!("{}", help());
        return Err(usage_error().into());
    };
    xtask::build_cache(&request, output)
        .map(|_| ())
        .map_err(Into::into)
}

fn run_stage_target(argument: Option<OsString>, output: &mut impl io::Write) -> CommandResult {
    let target = argument.expect("one release target was admitted");
    let Some(target) = target.to_str() else {
        eprintln!("{}", help());
        return Err(usage_error().into());
    };
    preflight_native_release(target).and_then(|()| {
        xtask::stage_target(target, release_environment(), output)
            .map(|_| ())
            .map_err(Into::into)
    })
}

fn run_commit_message_command(argument: Option<OsString>) -> CommandResult {
    let path = argument.expect("one commit-message path was admitted");
    match run_commit_message(Path::new(&path)) {
        Ok(true) => {
            eprintln!(
                concat!(
                    "spice commit-msg: auto-folded commit message body lines; ",
                    "keep body prose wrapped at {} chars"
                ),
                xtask::COMMIT_MESSAGE_WRAP_LIMIT
            );
            Ok(())
        }
        Ok(false) => Ok(()),
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
struct UsageError;

impl std::fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid cargo xtask invocation")
    }
}

impl std::error::Error for UsageError {}

fn usage_error() -> UsageError {
    UsageError
}

fn run_install(arguments: Vec<OsString>) -> ExitCode {
    let Some(request) = install_request(&arguments) else {
        eprintln!("{}", help());
        return ExitCode::from(USAGE_EXIT);
    };
    let mut output = io::stdout().lock();
    match xtask::refresh(&request, &release_environment(), &mut output) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Read one `cargo xtask install` invocation, or refuse the whole spelling.
fn install_request(arguments: &[OsString]) -> Option<xtask::RefreshRequest> {
    let mut request = xtask::RefreshRequest {
        restart: true,
        ..xtask::RefreshRequest::default()
    };
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str()? {
            "--release" => request.release = true,
            "--no-resident" => request.restart = false,
            "--host" => request.host = Some(arguments.next()?.to_str()?.to_owned()),
            "--port" => request.port = Some(arguments.next()?.to_str()?.parse().ok()?),
            _ => return None,
        }
    }
    Some(request)
}

fn build_cache_request(arguments: &[OsString]) -> Option<xtask::BuildCacheRequest> {
    let mut request = xtask::BuildCacheRequest::default();
    let mut mode = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str()? {
            "--dry-run" => {
                if mode.replace(false).is_some() {
                    return None;
                }
                request.apply = false;
            }
            "--apply" => {
                if mode.replace(true).is_some() {
                    return None;
                }
                request.apply = true;
            }
            "--max-bytes" => {
                request.max_bytes =
                    Some(xtask::parse_byte_quantity(arguments.next()?.to_str()?).ok()?);
            }
            "--free-space-floor" => {
                request.free_space_floor =
                    Some(xtask::parse_byte_quantity(arguments.next()?.to_str()?).ok()?);
            }
            "--retain-live-worktrees" => {
                request.retain_live_worktrees = arguments.next()?.to_str()?.parse().ok()?;
            }
            _ => return None,
        }
    }
    Some(request)
}

fn run_proof_image(arguments: Vec<OsString>) -> ExitCode {
    let (operation, archive) = match arguments.as_slice() {
        [operation] if operation == OsStr::new("build") => ("build", None),
        [operation] if operation == OsStr::new("verify") => ("verify", None),
        [operation, archive] if operation == OsStr::new("build") => {
            ("build", Some(PathBuf::from(archive)))
        }
        [operation, archive] if operation == OsStr::new("load") => {
            ("load", Some(PathBuf::from(archive)))
        }
        [operation, archive] if operation == OsStr::new("verify") => {
            ("verify", Some(PathBuf::from(archive)))
        }
        _ => {
            eprintln!("{}", help());
            return ExitCode::from(USAGE_EXIT);
        }
    };
    let repository = match spice::repository::Repository::discover(Path::new(".")) {
        Ok(repository) => repository,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let archive = archive.unwrap_or_else(|| {
        repository
            .worktree_root()
            .join("target/proof-image/spice-proof-image.tar")
    });
    let result = match operation {
        "build" => xtask::build_proof_image(
            repository.worktree_root(),
            &archive,
            &mut stdout,
            &mut stderr,
        ),
        "load" => xtask::load_proof_image(
            repository.worktree_root(),
            &archive,
            &mut stdout,
            &mut stderr,
        ),
        "verify" => xtask::build_proof_image(
            repository.worktree_root(),
            &archive,
            &mut stdout,
            &mut stderr,
        )
        .and_then(|()| {
            xtask::load_proof_image(
                repository.worktree_root(),
                &archive,
                &mut stdout,
                &mut stderr,
            )
        }),
        _ => unreachable!("proof-image operation was admitted above"),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(error.exit_code())
        }
    }
}

fn preflight_native_release(target: &str) -> Result<(), Box<dyn std::error::Error>> {
    if target != "aarch64-apple-darwin" {
        return Ok(());
    }
    let repository = spice::repository::Repository::discover(Path::new("."))?;
    let config = spice::config::load_repository(&repository, &[])?;
    let runner = config.root().proof().runner().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "proof.runner is required before Darwin release staging",
        )
    })?;
    let deadlines = Deadlines::new(
        Duration::from_secs(60),
        Duration::from_secs(10),
        Duration::from_secs(10),
    )?;
    let limits = proof::Limits::new(deadlines, Duration::from_secs(5), 1024 * 1024)?;
    let admitted =
        proof::NativeRunner::attest(runner.as_path(), repository.worktree_root(), limits)?;
    admitted.preflight()?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
#[expect(
    clippy::disallowed_methods,
    reason = "the binary rim snapshots the environment before passing typed configuration inward"
)]
fn ambient_environment() -> Result<sill::Environment, sill::SpecError> {
    sill::Environment::new(env::vars_os())
}

#[cfg(not(target_arch = "wasm32"))]
fn workspace_tests(
    concurrency: OsString,
    output: &mut impl io::Write,
    shared_outputs: bool,
    prebuilt: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let concurrency = concurrency
        .to_str()
        .ok_or("workspace-test concurrency is not UTF-8")?
        .parse::<usize>()?;
    let environment = workspace_test_environment(shared_outputs)?;
    let execution = if prebuilt {
        let root = env::current_dir()?;
        xtask::run_prebuilt_shared_workspace_tests_at(&root, environment, concurrency, output)
    } else {
        xtask::run_workspace_tests(environment, concurrency, output)
    };
    execution.map(|_execution| ()).map_err(Into::into)
}

#[cfg(not(target_arch = "wasm32"))]
fn is_workspace_test_command(command: &OsStr) -> bool {
    [
        "__workspace-tests",
        "__workspace-tests-shared",
        "__workspace-tests-prebuilt-shared",
        "__workspace-tests-prebuild-shared",
    ]
    .into_iter()
    .any(|candidate| command == OsStr::new(candidate))
}

#[cfg(not(target_arch = "wasm32"))]
fn run_workspace_test_command(
    command: &OsStr,
    arguments: Vec<OsString>,
    output: &mut impl io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    match (command.to_str(), arguments.as_slice()) {
        (Some("__workspace-tests"), [concurrency]) => {
            workspace_tests(concurrency.clone(), output, false, false)
        }
        (Some("__workspace-tests-shared"), [concurrency]) => {
            workspace_tests(concurrency.clone(), output, true, false)
        }
        (Some("__workspace-tests-prebuilt-shared"), [concurrency]) => {
            workspace_tests(concurrency.clone(), output, true, true)
        }
        (Some("__workspace-tests-prebuild-shared"), []) => workspace_tests_prebuild(output),
        _ => Err("invalid internal workspace-test command arguments".into()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn workspace_tests_prebuild(output: &mut impl io::Write) -> Result<(), Box<dyn std::error::Error>> {
    let root = env::current_dir()?;
    let environment = workspace_test_environment(true)?;
    xtask::prebuild_shared_workspace_tests_at(&root, environment, output).map_err(Into::into)
}

#[cfg(not(target_arch = "wasm32"))]
fn workspace_test_environment(shared_outputs: bool) -> Result<sill::Environment, sill::SpecError> {
    let environment = ambient_environment()?;
    if !shared_outputs {
        return Ok(environment);
    }
    let mut entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<OsString, OsString>>();
    entries.insert(
        OsString::from("SPICE_WORKSPACE_TEST_SHARED_OUTPUTS"), // env-policy: not-an-env SPICE_WORKSPACE_TEST_SHARED_OUTPUTS
        OsString::from("1"),
    );
    sill::Environment::new(entries)
}

fn release_environment() -> xtask::ReleaseEnvironment {
    xtask::ReleaseEnvironment::new(
        bootstrap_environment("PATH"),
        bootstrap_environment("HOME"),
        bootstrap_environment("CARGO_HOME"),
        bootstrap_environment("RUSTUP_HOME"),
        bootstrap_environment("RUSTUP_TOOLCHAIN"),
        bootstrap_environment("TMPDIR"),
        bootstrap_environment("SDKROOT"),
        bootstrap_environment("DEVELOPER_DIR"),
        bootstrap_environment("CARGO_TARGET_DIR"),
        bootstrap_environment("RUSTC"),
        bootstrap_environment("CARGO_BUILD_RUSTC"),
    )
}

#[expect(
    clippy::disallowed_methods,
    reason = "the xtask binary rim owns ambient release-build acquisition"
)]
fn bootstrap_environment(name: impl AsRef<OsStr>) -> Option<OsString> {
    env::var_os(name)
}

fn run_commit_message(path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let repository = spice::repository::Repository::discover(Path::new("."))?;
    let resolved = spice::config::load_repository(&repository, &[])?;
    Ok(xtask::commit_message(
        path,
        resolved.root().policy().commit_message(),
    )?)
}

fn run_reference_transaction(state: &OsStr) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    if state == OsStr::new("prepared") {
        io::stdin().read_to_string(&mut input)?;
    }
    let repository = env::current_dir()?;
    xtask::reference_transaction(&repository, state, &input).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::help;

    #[test]
    fn help_exposes_the_ordinary_selection_and_explicit_battery_actions() {
        let help = help();
        assert!(help.contains("verify-action selection"));
        assert!(help.contains("verify-action battery"));
        assert!(help.contains("Neither is a completion verifier"));
    }

    #[test]
    fn help_exposes_every_shared_test_evidence_instrument() {
        let help = help();
        for instrument in ["suite-seam", "mutation-coverage", "test-subsumption"] {
            assert!(help.contains(instrument));
        }
    }
}
