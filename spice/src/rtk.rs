//! Optional RTK command rewriting with an unconditional native fallback.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::{Duration, Instant};

use sill::{
    Argv, Cancellation, Completion, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};

/// Exact configured RTK identity carried into supervised shell wrappers.
pub const EXECUTABLE_VARIABLE: &str = wire::SpiceVariable::RtkExecutable.as_str();
/// RTK's upstream history database override.
pub const HISTORY_VARIABLE: &str = "RTK_DB_PATH"; // env-policy: allow

const SELECTOR_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_DRAIN: Duration = Duration::from_millis(250);
const REAP_DEADLINE: Duration = Duration::from_millis(500);
const TERMINATE_GRACE: Duration = Duration::from_millis(100);
const MAX_SELECTOR_OUTPUT: usize = 64 * 1024;
const MAX_DIAGNOSTIC_EXECUTABLE_CHARS: usize = 160;
// Earlier RTK rewrote `rg` through a basic-dialect grep path and changed valid
// extended-regex answers. 0.45.0 is the first source release that routes this
// probe through its answer-preserving `rtk rg` protocol.
const MINIMUM_VERSION: [u32; 3] = [0, 45, 0];
const MINIMUM_VERSION_TEXT: &str = "0.45.0";
const PROTOCOL_PROBE: [&str; 2] = ["git", "status"];
// `+` distinguishes extended from basic regex and stdin keeps the probe
// checkout-independent: the written command counts one match, not zero.
const FIDELITY_PROBE: [&str; 4] = ["rg", "--count", "al+pha", "-"];
const FIDELITY_SUBJECT: &[u8] = b"alpha\nbeta\n";
const EXTENDED_REGEX_OPERATORS: &str = "|+?(){}";

/// Admission state shared by Doctor and supervised-lane launch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthState {
    Active,
    Missing,
    Obsolete,
    ProtocolInvalid,
    RewriteUnfaithful,
}

impl HealthState {
    /// Stable status vocabulary for actionable diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Missing => "missing",
            Self::Obsolete => "obsolete",
            Self::ProtocolInvalid => "protocol-invalid",
            Self::RewriteUnfaithful => "rewrite-unfaithful",
        }
    }
}

/// One fail-native verdict for the exact configured RTK executable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Health {
    state: HealthState,
    version: Option<String>,
    detail: String,
}

impl Health {
    /// Whether supervised shells may enable RTK rewriting.
    #[must_use]
    pub const fn active(&self) -> bool {
        matches!(self.state, HealthState::Active)
    }

    /// Stable admission state.
    #[must_use]
    pub const fn state(&self) -> HealthState {
        self.state
    }

    /// Validated RTK version, when version admission reached that far.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Source-grounded reason for this verdict.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Command selected for execution and an optional non-blocking degradation notice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decision {
    command: Vec<OsString>,
    diagnostic: Option<String>,
}

impl Decision {
    /// Final command argv, rewritten only after a valid RTK answer.
    #[must_use]
    pub fn command(&self) -> &[OsString] {
        &self.command
    }

    /// Why RTK degraded to native execution.
    #[must_use]
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectorResult {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Decide whether the exact configured executable may rewrite supervised commands.
#[must_use]
pub fn probe_health(executable: &OsStr, working: &Path) -> Health {
    let environment = match ambient_environment() {
        Ok(environment) => environment,
        Err(error) => {
            return health(
                HealthState::ProtocolInvalid,
                None,
                format!("invalid health-probe environment: {error}"),
            );
        }
    };
    probe_health_in_environment(executable, working, &environment)
}

/// Decide RTK admission under the exact environment a supervised lane will use.
#[must_use]
pub fn probe_health_in_environment(
    executable: &OsStr,
    working: &Path,
    environment: &Environment,
) -> Health {
    let started = Instant::now();
    probe_health_with(executable, |program, arguments, input| {
        let remaining = HEALTH_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err("RTK health exceeded 10s deadline".to_owned());
        }
        run_bounded(
            program,
            arguments,
            working,
            environment.clone(),
            input,
            remaining,
            "RTK health probe",
        )
    })
}

fn probe_health_with<F>(executable: &OsStr, mut invoke: F) -> Health
where
    F: FnMut(&OsStr, &[OsString], Option<&[u8]>) -> Result<SelectorResult, String>,
{
    let version_text = match admitted_version(executable, &mut invoke) {
        Ok(version) => version,
        Err(verdict) => return verdict,
    };
    let protocol_exit = match admitted_protocol(executable, &version_text, &mut invoke) {
        Ok(code) => code,
        Err(verdict) => return verdict,
    };
    let written = match answer_fidelity(executable, &version_text, &mut invoke) {
        Ok(count) => count,
        Err(verdict) => return verdict,
    };
    health(
        HealthState::Active,
        Some(version_text),
        format!(
            "rewrite protocol valid (exit {protocol_exit}); rewrite preserved the answer (counted {written})"
        ),
    )
}

fn admitted_version<F>(executable: &OsStr, invoke: &mut F) -> Result<String, Health>
where
    F: FnMut(&OsStr, &[OsString], Option<&[u8]>) -> Result<SelectorResult, String>,
{
    let version_result = match invoke(executable, &["--version".into()], None) {
        Ok(result) => result,
        Err(reason) => return Err(health(HealthState::Missing, None, reason)),
    };
    let version_output = if version_result.stdout.trim().is_empty() {
        version_result.stderr.trim()
    } else {
        version_result.stdout.trim()
    };
    let Some(version) = (version_result.code == 0)
        .then(|| parse_version(version_output))
        .flatten()
    else {
        return Err(health(
            HealthState::ProtocolInvalid,
            None,
            format!("could not validate RTK version from {version_output:?}"),
        ));
    };
    let version_text = format!("{}.{}.{}", version[0], version[1], version[2]);
    if version < MINIMUM_VERSION {
        return Err(health(
            HealthState::Obsolete,
            Some(version_text.clone()),
            format!("RTK {version_text} is older than required {MINIMUM_VERSION_TEXT}"),
        ));
    }
    Ok(version_text)
}

fn admitted_protocol<F>(executable: &OsStr, version: &str, invoke: &mut F) -> Result<i32, Health>
where
    F: FnMut(&OsStr, &[OsString], Option<&[u8]>) -> Result<SelectorResult, String>,
{
    let protocol_arguments = rewrite_arguments(&PROTOCOL_PROBE);
    let protocol = match invoke(executable, &protocol_arguments, None) {
        Ok(result) => result,
        Err(reason) => {
            return Err(health(
                HealthState::ProtocolInvalid,
                Some(version.to_owned()),
                format!("rewrite protocol probe failed: {reason}"),
            ));
        }
    };
    let protocol_stdout = protocol.stdout.trim();
    if !matches!(protocol.code, 0 | 3) || protocol_stdout.is_empty() {
        return Err(health(
            HealthState::ProtocolInvalid,
            Some(version.to_owned()),
            format!(
                "rewrite probe returned exit {} with {} stdout; expected exit 0 or 3 with nonempty stdout",
                protocol.code,
                if protocol_stdout.is_empty() {
                    "empty"
                } else {
                    "nonempty"
                }
            ),
        ));
    }
    Ok(protocol.code)
}

fn answer_fidelity<F>(executable: &OsStr, version: &str, invoke: &mut F) -> Result<u64, Health>
where
    F: FnMut(&OsStr, &[OsString], Option<&[u8]>) -> Result<SelectorResult, String>,
{
    let written = match search_count(
        invoke,
        OsStr::new(FIDELITY_PROBE[0]),
        &FIDELITY_PROBE[1..]
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>(),
    ) {
        Ok(count) => count,
        Err(reason) => {
            return Err(health(
                HealthState::ProtocolInvalid,
                Some(version.to_owned()),
                format!("written fidelity probe could not be verified: {reason}"),
            ));
        }
    };
    let rewritten = rewritten_fidelity_count(executable, version, invoke)?;
    if rewritten != written {
        return Err(health(
            HealthState::RewriteUnfaithful,
            Some(version.to_owned()),
            format!(
                "rewriting {} changed its answer: as written it counted {written}, rewritten it counted {rewritten}",
                FIDELITY_PROBE.join(" ")
            ),
        ));
    }
    Ok(written)
}

fn rewritten_fidelity_count<F>(
    executable: &OsStr,
    version: &str,
    invoke: &mut F,
) -> Result<u64, Health>
where
    F: FnMut(&OsStr, &[OsString], Option<&[u8]>) -> Result<SelectorResult, String>,
{
    let fidelity_arguments = rewrite_arguments(&FIDELITY_PROBE);
    let fidelity = match invoke(executable, &fidelity_arguments, None) {
        Ok(result) if matches!(result.code, 0 | 3) && !result.stdout.trim().is_empty() => result,
        Ok(result) => {
            return Err(health(
                HealthState::ProtocolInvalid,
                Some(version.to_owned()),
                format!(
                    "fidelity rewrite returned exit {} with {} stdout",
                    result.code,
                    if result.stdout.trim().is_empty() {
                        "empty"
                    } else {
                        "nonempty"
                    }
                ),
            ));
        }
        Err(reason) => {
            return Err(health(
                HealthState::ProtocolInvalid,
                Some(version.to_owned()),
                format!("fidelity rewrite probe failed: {reason}"),
            ));
        }
    };
    let rewritten = fidelity.stdout.trim();
    if let Err(reason) = rewrite_fidelity(
        &FIDELITY_PROBE
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>(),
        rewritten,
    ) {
        return Err(health(
            HealthState::RewriteUnfaithful,
            Some(version.to_owned()),
            format!("fidelity rewrite was unsafe: {reason}"),
        ));
    }
    let mut parsed = match shell_words::split(rewritten) {
        Ok(parsed) if !parsed.is_empty() => parsed,
        Ok(_) => {
            return Err(health(
                HealthState::ProtocolInvalid,
                Some(version.to_owned()),
                "fidelity rewrite returned empty argv",
            ));
        }
        Err(error) => {
            return Err(health(
                HealthState::ProtocolInvalid,
                Some(version.to_owned()),
                format!("fidelity rewrite returned malformed argv: {error}"),
            ));
        }
    };
    remap_direct_frontend(&mut parsed, executable);
    let rewritten_program = OsStr::new(&parsed[0]);
    let rewritten_arguments = parsed[1..].iter().map(OsString::from).collect::<Vec<_>>();
    search_count(invoke, rewritten_program, &rewritten_arguments).map_err(|reason| {
        health(
            HealthState::ProtocolInvalid,
            Some(version.to_owned()),
            format!("rewritten fidelity probe could not be verified: {reason}"),
        )
    })
}

fn health(state: HealthState, version: Option<String>, detail: impl Into<String>) -> Health {
    Health {
        state,
        version,
        detail: detail.into(),
    }
}

fn rewrite_arguments(probe: &[&str]) -> Vec<OsString> {
    ["rewrite", "--"]
        .into_iter()
        .chain(probe.iter().copied())
        .map(OsString::from)
        .collect()
}

fn parse_version(output: &str) -> Option<[u32; 3]> {
    let mut words = output.split_whitespace();
    while let Some(word) = words.next() {
        if !word.eq_ignore_ascii_case("rtk") {
            continue;
        }
        let mut parts = words.next()?.split('.');
        let version = [
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ];
        return parts.next().is_none().then_some(version);
    }
    None
}

fn search_count<F>(invoke: &mut F, program: &OsStr, arguments: &[OsString]) -> Result<u64, String>
where
    F: FnMut(&OsStr, &[OsString], Option<&[u8]>) -> Result<SelectorResult, String>,
{
    let result = invoke(program, arguments, Some(FIDELITY_SUBJECT))?;
    let stdout = result.stdout.trim();
    if let Ok(count) = stdout.parse() {
        return Ok(count);
    }
    if result.code == 1 && stdout.is_empty() {
        return Ok(0);
    }
    Err(format!(
        "search returned exit {} without one numeric count",
        result.code
    ))
}

/// Ask the configured RTK executable to rewrite one command.
#[must_use]
pub fn rewrite(command: &[OsString], executable: &OsStr) -> Decision {
    rewrite_with(command, executable, |arguments| {
        run_selector(executable, arguments)
    })
}

fn rewrite_with<F>(command: &[OsString], executable: &OsStr, invoke: F) -> Decision
where
    F: FnOnce(&[String]) -> Result<SelectorResult, String>,
{
    let native = || Decision {
        command: command.to_vec(),
        diagnostic: None,
    };
    let Some(arguments) = selector_arguments(command) else {
        return degraded(command, executable, "command contains non-UTF-8 arguments");
    };
    let result = match invoke(&arguments) {
        Ok(result) => result,
        Err(reason) => return degraded(command, executable, &reason),
    };
    let rewritten = result.stdout.trim();
    if result.code == 1 && rewritten.is_empty() {
        return native();
    }
    if !matches!(result.code, 0 | 3) || rewritten.is_empty() {
        return degraded(
            command,
            executable,
            &format!(
                "rewrite returned exit {} with {} stdout",
                result.code,
                if rewritten.is_empty() {
                    "empty"
                } else {
                    "nonempty"
                }
            ),
        );
    }
    if let Err(reason) = rewrite_fidelity(command, rewritten) {
        return degraded(command, executable, &reason);
    }

    let Some(shell_index) = shell_command_index(command) else {
        let mut parsed = match shell_words::split(rewritten) {
            Ok(parsed) if !parsed.is_empty() => parsed,
            Ok(_) => return degraded(command, executable, "rewrite returned empty argv"),
            Err(error) => {
                return degraded(
                    command,
                    executable,
                    &format!("rewrite returned malformed argv: {error}"),
                );
            }
        };
        remap_direct_frontend(&mut parsed, executable);
        return Decision {
            command: parsed.into_iter().map(OsString::from).collect(),
            diagnostic: None,
        };
    };

    let mut selected = command.to_vec();
    selected[shell_index] = remap_shell_frontend(rewritten, executable).into();
    Decision {
        command: selected,
        diagnostic: None,
    }
}

fn rewrite_fidelity(command: &[OsString], rewritten: &str) -> Result<(), String> {
    let arguments = selector_arguments(command)
        .ok_or_else(|| "command contains non-UTF-8 arguments".to_owned())?;
    let written = if arguments.len() == 1 {
        shell_words::split(&arguments[0])
            .map_err(|error| format!("written shell command could not be parsed: {error}"))?
    } else {
        arguments.clone()
    };
    let substituted = shell_words::split(rewritten)
        .map_err(|error| format!("rewrite returned malformed argv: {error}"))?;
    if substituted.is_empty() {
        return Err("rewrite returned empty argv".to_owned());
    }
    let operators = narrowed_regex_operators(&written, &substituted);
    if !operators.is_empty() {
        return Err(format!(
            "rewrite narrowed the written regex dialect for operators {operators:?}"
        ));
    }
    if arguments.len() > 1
        && let Some(index) = split_argument(&arguments, &substituted)
    {
        return Err(format!("rewrite split or unquoted argument {index}"));
    }
    Ok(())
}

fn narrowed_regex_operators(written: &[String], substituted: &[String]) -> String {
    let written_extended = written
        .iter()
        .any(|word| matches!(word.as_str(), "rg" | "egrep") || requests_extended_regex(word));
    let substituted_basic = substituted
        .iter()
        .any(|word| matches!(word.as_str(), "grep" | "fgrep"));
    if !written_extended
        || !substituted_basic
        || substituted.iter().any(|word| requests_extended_regex(word))
    {
        return String::new();
    }
    EXTENDED_REGEX_OPERATORS
        .chars()
        .filter(|operator| {
            written
                .iter()
                .any(|word| !word.starts_with('-') && word.contains(*operator))
        })
        .collect()
}

fn requests_extended_regex(word: &str) -> bool {
    matches!(word, "-E" | "--extended-regexp")
        || (word.starts_with('-') && !word.starts_with("--") && word.contains('E'))
}

fn split_argument(arguments: &[String], substituted: &[String]) -> Option<usize> {
    arguments.iter().enumerate().find_map(|(index, argument)| {
        if substituted.iter().any(|word| word == argument) {
            return None;
        }
        let split_on_whitespace = argument.split_whitespace().count() > 1;
        (split_on_whitespace || split_across_words(argument, substituted)).then_some(index)
    })
}

fn split_across_words(argument: &str, substituted: &[String]) -> bool {
    let target = argument.split_whitespace().collect::<String>();
    for start in 0..substituted.len().saturating_sub(1) {
        let mut joined = substituted[start].clone();
        for word in &substituted[start + 1..] {
            joined.push_str(word);
            if joined.len() > target.len() {
                break;
            }
            if joined == target {
                return true;
            }
        }
    }
    false
}

fn selector_arguments(command: &[OsString]) -> Option<Vec<String>> {
    if let Some(index) = shell_command_index(command) {
        return command[index].to_str().map(|text| vec![text.to_owned()]);
    }
    command
        .iter()
        .map(|argument| argument.to_str().map(str::to_owned))
        .collect()
}

fn shell_command_index(command: &[OsString]) -> Option<usize> {
    if command.len() < 3 || !matches!(command[1].to_str(), Some("-c" | "-lc")) {
        return None;
    }
    let program = Path::new(&command[0]).file_name()?.to_str()?;
    matches!(program, "bash" | "dash" | "sh" | "stage.sh" | "zsh").then_some(2)
}

fn remap_direct_frontend(command: &mut [String], executable: &OsStr) {
    if command.first().is_some_and(|word| word == "rtk")
        && let Some(executable) = executable.to_str()
    {
        command[0] = executable.to_owned();
    }
}

fn remap_shell_frontend(command: &str, executable: &OsStr) -> String {
    let Some(executable) = executable.to_str() else {
        return command.to_owned();
    };
    if executable == "rtk" {
        return command.to_owned();
    }
    let quoted = shell_quote(executable);
    if command == "rtk" {
        return quoted;
    }
    command
        .strip_prefix("rtk ")
        .map_or_else(|| command.to_owned(), |tail| format!("{quoted} {tail}"))
}

fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

fn degraded(command: &[OsString], executable: &OsStr, reason: &str) -> Decision {
    let mut display = executable.to_string_lossy().into_owned();
    if display.chars().count() > MAX_DIAGNOSTIC_EXECUTABLE_CHARS {
        display = display
            .chars()
            .take(MAX_DIAGNOSTIC_EXECUTABLE_CHARS - 3)
            .collect::<String>();
        display.push_str("...");
    }
    Decision {
        command: command.to_vec(),
        diagnostic: Some(format!(
            "spice run: RTK rewrite degraded to native executable={display:?}: {reason}"
        )),
    }
}

fn run_selector(executable: &OsStr, arguments: &[String]) -> Result<SelectorResult, String> {
    let selector_arguments = ["rewrite".to_owned(), "--".to_owned()]
        .into_iter()
        .chain(arguments.iter().cloned())
        .map(OsString::from)
        .collect::<Vec<_>>();
    let working =
        env::current_dir().map_err(|error| format!("working directory unavailable: {error}"))?;
    run_bounded(
        executable,
        &selector_arguments,
        &working,
        ambient_environment().map_err(|error| format!("invalid selector environment: {error}"))?,
        None,
        SELECTOR_TIMEOUT,
        "optional RTK rewrite selector",
    )
}

fn run_bounded(
    executable: &OsStr,
    arguments: &[OsString],
    working: &Path,
    environment: Environment,
    input: Option<&[u8]>,
    runtime: Duration,
    label: &str,
) -> Result<SelectorResult, String> {
    let capture = StreamPolicy::Capture {
        limit: MAX_SELECTOR_OUTPUT,
    };
    let spec = Spec {
        argv: Argv::new(executable.to_owned(), arguments.iter().cloned())
            .map_err(|error| format!("invalid selector argv: {error}"))?,
        cwd: working.to_path_buf(),
        environment,
        group: ProcessGroup::New,
        deadlines: Deadlines::new(runtime, OUTPUT_DRAIN, REAP_DEADLINE)
            .map_err(|error| format!("invalid selector deadlines: {error}"))?,
        wait: WaitPolicy::new(label, Cancellation::Never, RestartDisposition::Never)
            .map_err(|error| format!("invalid selector wait policy: {error}"))?,
        output: Output::new(
            if input.is_some() {
                StdinPolicy::Pipe
            } else {
                StdinPolicy::Null
            },
            capture,
            capture,
        ),
        redaction: Redaction::None,
        grace: TERMINATE_GRACE,
    };
    let mut child =
        sill::spawn(spec).map_err(|error| format!("selector launch failed: {error}"))?;
    if let Some(input) = input {
        child
            .write_stdin(input)
            .map_err(|error| format!("selector stdin failed: {error}"))?;
    }
    let outcome = child
        .wait()
        .map_err(|error| format!("selector process boundary failed: {error}"))?;
    if !outcome.outputs_drained() {
        return Err("selector descendants retained an output pipe".to_owned());
    }
    match outcome.completion() {
        Completion::TimedOut(_) => {
            return Err(format!(
                "{label} exceeded {}ms deadline",
                runtime.as_millis()
            ));
        }
        Completion::Cancelled(_) => return Err("selector was cancelled".to_owned()),
        Completion::Exited(_) => {}
    }
    let stdout = captured_text(outcome.stdout(), "stdout")?;
    let stderr = captured_text(outcome.stderr(), "stderr")?;
    Ok(SelectorResult {
        code: outcome.completion().status().code().unwrap_or(-1),
        stdout,
        stderr,
    })
}

fn captured_text(output: Option<&sill::CapturedOutput>, stream: &str) -> Result<String, String> {
    let output = output.ok_or_else(|| format!("selector {stream} was unavailable"))?;
    if output.truncated() {
        return Err(format!(
            "selector {stream} exceeded {MAX_SELECTOR_OUTPUT} bytes"
        ));
    }
    if !output.complete() {
        return Err(format!("selector {stream} did not close"));
    }
    String::from_utf8(output.bytes().to_vec())
        .map_err(|_| format!("selector {stream} was not UTF-8"))
}

#[expect(
    clippy::disallowed_methods,
    reason = "the external-process rim captures the wrapper environment once"
)]
fn ambient_environment() -> Result<Environment, sill::SpecError> {
    Environment::new(env::vars_os())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn answer(code: i32, stdout: &str) -> Result<SelectorResult, String> {
        Ok(result(code, stdout))
    }

    fn result(code: i32, stdout: &str) -> SelectorResult {
        SelectorResult {
            code,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        }
    }

    #[test]
    fn health_admits_the_exact_supported_answer_preserving_executable() {
        let executable = OsStr::new("/opt/RTK Tools/rtk");
        let mut stage = 0;
        let observed = probe_health_with(executable, |program, arguments, input| {
            let arguments = arguments
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let response = match stage {
                0 => {
                    assert_eq!(program, executable);
                    assert_eq!(arguments, ["--version"]);
                    assert_eq!(input, None);
                    result(0, "rtk 0.45.0\n")
                }
                1 => {
                    assert_eq!(program, executable);
                    assert_eq!(arguments, ["rewrite", "--", "git", "status"]);
                    assert_eq!(input, None);
                    result(3, "rtk git status --short\n")
                }
                2 => {
                    assert_eq!(program, "rg");
                    assert_eq!(arguments, ["--count", "al+pha", "-"]);
                    assert_eq!(input, Some(FIDELITY_SUBJECT));
                    result(0, "1\n")
                }
                3 => {
                    assert_eq!(program, executable);
                    assert_eq!(arguments, ["rewrite", "--", "rg", "--count", "al+pha", "-"]);
                    assert_eq!(input, None);
                    result(0, "rtk grep --count -E al+pha -\n")
                }
                4 => {
                    assert_eq!(program, executable);
                    assert_eq!(arguments, ["grep", "--count", "-E", "al+pha", "-"]);
                    assert_eq!(input, Some(FIDELITY_SUBJECT));
                    result(0, "1\n")
                }
                _ => panic!("health performed an unexpected probe stage"),
            };
            stage += 1;
            Ok(response)
        });

        assert_eq!(stage, 5);
        assert_eq!(observed.state(), HealthState::Active);
        assert_eq!(observed.version(), Some("0.45.0"));
        assert!(observed.detail().contains("preserved the answer"));
    }

    #[test]
    fn health_refuses_missing_obsolete_and_protocol_invalid_executables() {
        let missing = probe_health_with(OsStr::new("missing-rtk"), |_, _, _| {
            Err("selector launch failed: not found".to_owned())
        });
        assert_eq!(missing.state(), HealthState::Missing);

        let obsolete = probe_health_with(OsStr::new("old-rtk"), |_, arguments, _| {
            assert_eq!(arguments, [OsString::from("--version")]);
            Ok(result(0, "rtk 0.44.9\n"))
        });
        assert_eq!(obsolete.state(), HealthState::Obsolete);
        assert_eq!(obsolete.version(), Some("0.44.9"));

        let mut stage = 0;
        let invalid = probe_health_with(OsStr::new("broken-rtk"), |_, _, _| {
            let response = if stage == 0 {
                result(0, "rtk 0.45.0\n")
            } else {
                result(1, "unexpected\n")
            };
            stage += 1;
            Ok(response)
        });
        assert_eq!(invalid.state(), HealthState::ProtocolInvalid);
        assert!(invalid.detail().contains("exit 1 with nonempty stdout"));
    }

    #[test]
    fn health_refuses_a_rewrite_that_changes_the_probe_answer() {
        let mut stage = 0;
        let observed = probe_health_with(OsStr::new("rtk"), |_, _, _| {
            let response = match stage {
                0 => result(0, "rtk 0.45.0\n"),
                1 => result(0, "rtk git status --short\n"),
                2 => result(0, "1\n"),
                3 => result(0, "rtk grep --count -E al+pha -\n"),
                4 => result(1, ""),
                _ => panic!("health performed an unexpected probe stage"),
            };
            stage += 1;
            Ok(response)
        });

        assert_eq!(observed.state(), HealthState::RewriteUnfaithful);
        assert!(observed.detail().contains("counted 1"));
        assert!(observed.detail().contains("counted 0"));
    }

    #[test]
    fn direct_and_shell_rewrites_remap_the_canonical_frontend() {
        let direct = words(&["git", "status"]);
        let selected = rewrite_with(&direct, OsStr::new("alternate-rtk"), |arguments| {
            assert_eq!(arguments, ["git", "status"]);
            answer(3, "rtk git status\n")
        });
        assert_eq!(
            selected.command(),
            words(&["alternate-rtk", "git", "status"])
        );
        assert_eq!(selected.diagnostic(), None);

        let shell = words(&["/installed/static/stage.sh", "-c", "git status"]);
        let selected = rewrite_with(&shell, OsStr::new("/opt/RTK Tools/rtk"), |arguments| {
            assert_eq!(arguments, ["git status"]);
            answer(0, "rtk git status\n")
        });
        assert_eq!(
            selected.command()[2],
            OsString::from("'/opt/RTK Tools/rtk' git status")
        );
    }

    #[test]
    fn no_match_and_every_invalid_answer_preserve_the_native_command() {
        let native = words(&["git", "status"]);
        let no_match = rewrite_with(&native, OsStr::new("rtk"), |_| answer(1, ""));
        assert_eq!(no_match.command(), native);
        assert_eq!(no_match.diagnostic(), None);

        for response in [
            answer(0, ""),
            answer(1, "rtk git status"),
            answer(9, "broken"),
        ] {
            let decision = rewrite_with(&native, OsStr::new("rtk"), |_| response);
            assert_eq!(decision.command(), native);
            assert!(
                decision
                    .diagnostic()
                    .is_some_and(|line| { line.contains("RTK rewrite degraded to native") })
            );
        }
        let unavailable = rewrite_with(&native, OsStr::new("missing-rtk"), |_| {
            Err("selector launch failed: not found".to_owned())
        });
        assert_eq!(unavailable.command(), native);
        assert!(
            unavailable
                .diagnostic()
                .is_some_and(|line| line.contains("missing-rtk"))
        );

        let long_executable = "x".repeat(4 * MAX_DIAGNOSTIC_EXECUTABLE_CHARS);
        let bounded = rewrite_with(&native, OsStr::new(&long_executable), |_| {
            Err("selector launch failed: not found".to_owned())
        });
        let diagnostic = bounded.diagnostic().expect("degradation is diagnosed");
        assert!(
            diagnostic.len() < MAX_DIAGNOSTIC_EXECUTABLE_CHARS * 2,
            "diagnostic was not bounded: {diagnostic}"
        );
        assert!(!diagnostic.contains(&long_executable));
    }

    #[test]
    fn malformed_direct_output_and_non_utf8_input_degrade_without_mutation() {
        let native = words(&["git", "status"]);
        let malformed = rewrite_with(&native, OsStr::new("rtk"), |_| {
            answer(3, "rtk 'unterminated")
        });
        assert_eq!(malformed.command(), native);
        assert!(malformed.diagnostic().is_some());

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;

            let native = vec![OsString::from_vec(vec![0xff])];
            let decision = rewrite_with(&native, OsStr::new("rtk"), |_| {
                panic!("non-UTF-8 command must not start RTK")
            });
            assert_eq!(decision.command(), native);
            assert!(decision.diagnostic().is_some());
        }
    }

    #[test]
    fn regex_narrowing_and_split_arguments_degrade_to_the_exact_native_argv() {
        for (native, rewritten, reason) in [
            (
                words(&["rg", "--count", "alpha|beta", "subject"]),
                "rtk grep --count alpha|beta subject",
                "regex dialect",
            ),
            (
                words(&["rg", "--count", "alpha|beta", "subject"]),
                "rtk rg --count alpha | beta subject",
                "split or unquoted",
            ),
            (
                words(&["rg", "needle", "path with spaces"]),
                "rtk rg needle path with spaces",
                "split or unquoted",
            ),
        ] {
            let decision = rewrite_with(&native, OsStr::new("rtk"), |_| answer(3, rewritten));
            assert_eq!(decision.command(), native);
            assert!(
                decision
                    .diagnostic()
                    .is_some_and(|diagnostic| diagnostic.contains(reason)),
                "unexpected diagnostic: {:?}",
                decision.diagnostic()
            );
        }
    }

    #[test]
    fn fidelity_accepts_extended_regex_and_quoted_argument_boundaries() {
        let native = words(&["rg", "--count", "alpha|beta", "path with spaces"]);
        let decision = rewrite_with(&native, OsStr::new("alternate-rtk"), |_| {
            answer(3, "rtk grep --count -E 'alpha|beta' 'path with spaces'")
        });

        assert_eq!(
            decision.command(),
            words(&[
                "alternate-rtk",
                "grep",
                "--count",
                "-E",
                "alpha|beta",
                "path with spaces"
            ])
        );
        assert_eq!(decision.diagnostic(), None);
    }
}
