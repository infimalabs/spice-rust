//! Optional RTK command rewriting with an unconditional native fallback.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;

use sill::{
    Argv, Cancellation, Completion, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};

/// Exact configured RTK identity carried into supervised shell wrappers.
pub const EXECUTABLE_VARIABLE: &str = wire::SpiceVariable::RtkExecutable.as_str();
/// RTK's upstream history database override.
pub const HISTORY_VARIABLE: &str = "RTK_DB_PATH"; // env-policy: allow

const SELECTOR_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_DRAIN: Duration = Duration::from_millis(250);
const REAP_DEADLINE: Duration = Duration::from_millis(500);
const TERMINATE_GRACE: Duration = Duration::from_millis(100);
const MAX_SELECTOR_OUTPUT: usize = 64 * 1024;

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
    Decision {
        command: command.to_vec(),
        diagnostic: Some(format!(
            "spice run: RTK rewrite degraded to native executable={executable:?}: {reason}"
        )),
    }
}

fn run_selector(executable: &OsStr, arguments: &[String]) -> Result<SelectorResult, String> {
    let selector_arguments = ["rewrite".to_owned(), "--".to_owned()]
        .into_iter()
        .chain(arguments.iter().cloned());
    let capture = StreamPolicy::Capture {
        limit: MAX_SELECTOR_OUTPUT,
    };
    let spec = Spec {
        argv: Argv::new(executable.to_owned(), selector_arguments)
            .map_err(|error| format!("invalid selector argv: {error}"))?,
        cwd: env::current_dir()
            .map_err(|error| format!("working directory unavailable: {error}"))?,
        environment: ambient_environment()
            .map_err(|error| format!("invalid selector environment: {error}"))?,
        group: ProcessGroup::New,
        deadlines: Deadlines::new(SELECTOR_TIMEOUT, OUTPUT_DRAIN, REAP_DEADLINE)
            .map_err(|error| format!("invalid selector deadlines: {error}"))?,
        wait: WaitPolicy::new(
            "optional RTK rewrite selector",
            Cancellation::Never,
            RestartDisposition::Never,
        )
        .map_err(|error| format!("invalid selector wait policy: {error}"))?,
        output: Output::new(StdinPolicy::Null, capture, capture),
        redaction: Redaction::None,
        grace: TERMINATE_GRACE,
    };
    let child = sill::spawn(spec).map_err(|error| format!("selector launch failed: {error}"))?;
    let outcome = child
        .wait()
        .map_err(|error| format!("selector process boundary failed: {error}"))?;
    if !outcome.outputs_drained() {
        return Err("selector descendants retained an output pipe".to_owned());
    }
    match outcome.completion() {
        Completion::TimedOut(_) => return Err("selector exceeded 5s deadline".to_owned()),
        Completion::Cancelled(_) => return Err("selector was cancelled".to_owned()),
        Completion::Exited(_) => {}
    }
    let stdout = outcome
        .stdout()
        .ok_or_else(|| "selector stdout was unavailable".to_owned())?;
    if stdout.truncated() {
        return Err(format!(
            "selector stdout exceeded {MAX_SELECTOR_OUTPUT} bytes"
        ));
    }
    if !stdout.complete() {
        return Err("selector stdout did not close".to_owned());
    }
    let stdout = String::from_utf8(stdout.bytes().to_vec())
        .map_err(|_| "selector stdout was not UTF-8".to_owned())?;
    Ok(SelectorResult {
        code: outcome.completion().status().code().unwrap_or(-1),
        stdout,
    })
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
        Ok(SelectorResult {
            code,
            stdout: stdout.to_owned(),
        })
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
}
