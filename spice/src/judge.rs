//! One bounded external YES/NO adjudication process shared by every question.

use std::env;
use std::time::Duration;

use sill::{
    Argv, Cancellation, CancellationToken, Completion, Deadlines, Environment, Output,
    ProcessGroup, Redaction, RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};

const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REPLY_BYTES: usize = 4 * 1024;
const OUTPUT_DRAIN: Duration = Duration::from_millis(250);
const REAP_DEADLINE: Duration = Duration::from_millis(500);
const TERMINATE_GRACE: Duration = Duration::from_millis(100);

/// One semantically complete framing, identified independently of its order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Prompt {
    framing: u8,
    text: String,
}

impl Prompt {
    pub(crate) fn new(framing: u8, text: String) -> Self {
        Self { framing, text }
    }

    pub(crate) const fn framing(&self) -> u8 {
        self.framing
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

/// One bounded invocation retained beside a decisive answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Attempt {
    framing: u8,
    reply: String,
    answer: Option<bool>,
}

impl Attempt {
    pub(crate) const fn framing(&self) -> u8 {
        self.framing
    }

    pub(crate) fn reply(&self) -> &str {
        &self.reply
    }

    pub(crate) const fn answer(&self) -> Option<bool> {
        self.answer
    }
}

/// The first unambiguous answer and every attempt that led to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Decision {
    confirmed: bool,
    attempts: Vec<Attempt>,
}

impl Decision {
    pub(crate) const fn confirmed(&self) -> bool {
        self.confirmed
    }

    pub(crate) fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }
}

/// Failure to obtain one decisive answer inside the declared boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Error {
    reason: &'static str,
    detail: String,
}

impl Error {
    #[cfg(test)]
    pub(crate) const fn reason(&self) -> &'static str {
        self.reason
    }

    #[cfg(test)]
    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
}

/// The sole executable/model boundary used by adjudication consumers.
#[derive(Debug)]
pub(crate) struct Process {
    binary: String,
    timeout: Duration,
    cancellation: CancellationToken,
}

impl Default for Process {
    fn default() -> Self {
        Self {
            binary: crate::cli::JUDGE_EXECUTABLE.to_owned(),
            timeout: ATTEMPT_TIMEOUT,
            cancellation: CancellationToken::new(),
        }
    }
}

impl Process {
    /// Ask ordered equivalent framings until exactly one normalized answer lands.
    pub(crate) fn adjudicate(&mut self, prompts: &[Prompt]) -> Result<Decision, Error> {
        let binary = self.binary.clone();
        let timeout = self.timeout;
        let cancellation = self.cancellation.clone();
        adjudicate_with(prompts, |prompt| {
            invoke_with(&binary, prompt, timeout, cancellation.clone())
        })
    }

    #[cfg(test)]
    pub(crate) fn with(binary: String, timeout: Duration) -> Self {
        Self {
            binary,
            timeout,
            cancellation: CancellationToken::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_contract(
        binary: String,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            binary,
            timeout,
            cancellation,
        }
    }

    pub(crate) fn with_cancellation(cancellation: CancellationToken) -> Self {
        Self {
            cancellation,
            ..Self::default()
        }
    }
}

pub(crate) fn adjudicate_with(
    prompts: &[Prompt],
    mut invoke_prompt: impl FnMut(&str) -> Result<String, Error>,
) -> Result<Decision, Error> {
    if prompts.is_empty() {
        return Err(failure_error(
            "judge_error",
            "an adjudication question declared no framing",
        ));
    }
    let mut attempts = Vec::with_capacity(prompts.len());
    for prompt in prompts {
        match invoke_prompt(prompt.text()) {
            Ok(raw) => {
                let reply = compact(&raw);
                let answer = parse_yes_no(&raw);
                attempts.push(Attempt {
                    framing: prompt.framing(),
                    reply,
                    answer,
                });
                if let Some(confirmed) = answer {
                    return Ok(Decision {
                        confirmed,
                        attempts,
                    });
                }
            }
            Err(error) if error.reason == "ambiguous" => attempts.push(Attempt {
                framing: prompt.framing(),
                reply: compact(&error.detail),
                answer: None,
            }),
            Err(error) => return Err(error),
        }
    }
    Err(Error {
        reason: "ambiguous",
        detail: format!(
            "judge did not return one YES/NO in {} attempts: {}",
            prompts.len(),
            attempts
                .iter()
                .map(|attempt| format!("{}:{:?}", attempt.framing, attempt.reply))
                .collect::<Vec<_>>()
                .join(" | ")
        ),
    })
}

#[cfg(test)]
pub(crate) fn invoke(binary: &str, prompt: &str, timeout: Duration) -> Result<String, Error> {
    invoke_with(binary, prompt, timeout, CancellationToken::new())
}

fn invoke_with(
    binary: &str,
    prompt: &str,
    timeout: Duration,
    cancellation: CancellationToken,
) -> Result<String, Error> {
    let spec = process_spec(binary, timeout, cancellation)?;
    let mut child = sill::spawn(spec).map_err(|error| Error {
        reason: "unavailable",
        detail: format!("could not launch {binary}: {error}"),
    })?;
    child
        .write_stdin(prompt.as_bytes())
        .map_err(|error| Error {
            reason: "judge_error",
            detail: format!("could not write judge prompt: {error}"),
        })?;
    let outcome = child.wait().map_err(|error| Error {
        reason: "judge_error",
        detail: format!("judge process boundary failed: {error}"),
    })?;
    if !outcome.outputs_drained() {
        return Err(failure_error(
            "judge_error",
            "judge descendants retained an output pipe past the drain deadline",
        ));
    }
    match outcome.completion() {
        Completion::TimedOut(_) => {
            return Err(Error {
                reason: "timeout",
                detail: format!("judge exceeded {}ms", timeout.as_millis()),
            });
        }
        Completion::Cancelled(_) => {
            return Err(failure_error("cancelled", "judge request was cancelled"));
        }
        Completion::Exited(_) => {}
    }
    let stdout = captured_output(outcome.stdout(), "stdout")?;
    let stderr = captured_output(outcome.stderr(), "stderr")?;
    let status = outcome.completion().status();
    if !status.success() {
        return Err(Error {
            reason: "unavailable",
            detail: format!(
                "{binary} exited with {}{}",
                status
                    .code()
                    .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {}", compact(&stderr))
                },
            ),
        });
    }
    Ok(stdout)
}

fn process_spec(
    binary: &str,
    timeout: Duration,
    cancellation: CancellationToken,
) -> Result<Spec, Error> {
    let capture = StreamPolicy::Capture {
        limit: MAX_REPLY_BYTES,
    };
    Ok(Spec {
        argv: Argv::new(binary, std::iter::empty::<String>()).map_err(spec_error)?,
        cwd: env::current_dir().map_err(|error| Error {
            reason: "judge_error",
            detail: format!("could not resolve judge working directory: {error}"),
        })?,
        environment: ambient_environment().map_err(spec_error)?,
        group: ProcessGroup::New,
        deadlines: Deadlines::new(timeout, OUTPUT_DRAIN, REAP_DEADLINE).map_err(spec_error)?,
        wait: WaitPolicy::new(
            "shared optional judge",
            Cancellation::Token(cancellation),
            RestartDisposition::Never,
        )
        .map_err(spec_error)?,
        output: Output::new(StdinPolicy::Pipe, capture, capture),
        redaction: Redaction::All,
        grace: TERMINATE_GRACE,
    })
}

#[expect(
    clippy::disallowed_methods,
    reason = "the shared external-process rim captures the caller environment once"
)]
fn ambient_environment() -> Result<Environment, sill::SpecError> {
    Environment::new(env::vars_os())
}

fn spec_error(error: sill::SpecError) -> Error {
    Error {
        reason: "judge_error",
        detail: format!("judge process contract is invalid: {error}"),
    }
}

fn captured_output(output: Option<&sill::CapturedOutput>, stream: &str) -> Result<String, Error> {
    let output = output.ok_or_else(|| Error {
        reason: "judge_error",
        detail: format!("judge {stream} capture was unavailable"),
    })?;
    if output.truncated() {
        return Err(Error {
            reason: "ambiguous",
            detail: format!("judge {stream} exceeded {MAX_REPLY_BYTES} bytes"),
        });
    }
    if !output.complete() {
        return Err(Error {
            reason: "judge_error",
            detail: format!("judge {stream} did not close within the output deadline"),
        });
    }
    String::from_utf8(output.bytes().to_vec()).map_err(|error| Error {
        reason: "ambiguous",
        detail: format!("judge {stream} was not UTF-8: {error}"),
    })
}

pub(crate) fn parse_yes_no(raw: &str) -> Option<bool> {
    let compact = compact(raw);
    let normalized = compact
        .trim_end_matches(['.', '!'])
        .trim()
        .to_ascii_uppercase();
    match normalized.as_str() {
        "YES" => Some(true),
        "NO" => Some(false),
        _ => None,
    }
}

pub(crate) fn compact(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn failure_error(reason: &'static str, detail: &str) -> Error {
    Error {
        reason,
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_SCRIPT: AtomicU64 = AtomicU64::new(0);
    // These scripts finish immediately once scheduled. Their bound protects
    // the test process from a fixture defect; the separate 40ms case below
    // proves the production timeout behavior. Leave enough scheduling room
    // for an exhaustive gate running hundreds of process tests concurrently.
    const PROCESS_RUNTIME: Duration = Duration::from_secs(30);

    struct Script(PathBuf);

    impl Script {
        fn new(body: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "spice-judge-test-{}-{}",
                std::process::id(),
                NEXT_SCRIPT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write judge fixture");
            let mut permissions = fs::metadata(&path)
                .expect("judge fixture metadata")
                .permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&path, permissions).expect("make judge fixture executable");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Script {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn prompts() -> Vec<Prompt> {
        vec![
            Prompt::new(2, "third semantic framing".to_owned()),
            Prompt::new(0, "first semantic framing".to_owned()),
            Prompt::new(1, "second semantic framing".to_owned()),
        ]
    }

    #[test]
    fn malformed_replies_retry_in_declared_order_and_stop_on_one_answer() {
        let mut invoked = Vec::new();
        let mut replies = ["Answer: maybe", "YES"].into_iter();

        let decision = adjudicate_with(&prompts(), |prompt| {
            invoked.push(prompt.to_owned());
            Ok(replies.next().expect("bounded fixture reply").to_owned())
        })
        .expect("second framing is decisive");

        assert!(decision.confirmed());
        assert_eq!(
            invoked,
            ["third semantic framing", "first semantic framing"]
        );
        assert_eq!(
            decision
                .attempts()
                .iter()
                .map(Attempt::framing)
                .collect::<Vec<_>>(),
            [2, 0]
        );
        assert_eq!(decision.attempts()[0].answer(), None);
        assert_eq!(decision.attempts()[1].answer(), Some(true));
    }

    #[test]
    fn ambiguity_exhausts_the_exact_declared_bound_without_interpretation() {
        let calls = std::cell::Cell::new(0);
        let error = adjudicate_with(&prompts(), |_| {
            calls.set(calls.get() + 1);
            Ok("YES and NO".to_owned())
        })
        .expect_err("mixed replies never become a verdict");

        assert_eq!(calls.get(), 3);
        assert_eq!(error.reason(), "ambiguous");
        assert!(error.detail().contains("3 attempts"));
    }

    #[test]
    fn unavailable_process_is_abandoned_at_the_shared_boundary() {
        let mut process = Process::with(
            "/definitely/missing/spice-judge".to_owned(),
            Duration::from_millis(20),
        );
        let error = process
            .adjudicate(&prompts()[..1])
            .expect_err("missing optional judge has no verdict");

        assert_eq!(error.reason(), "unavailable");
        assert!(error.detail().contains("could not launch"));
    }

    #[test]
    fn decisive_process_reads_the_complete_prompt_and_closes_both_pipes() {
        let script = Script::new("input=$(cat); test -n \"$input\"; printf 'YES\\n'");

        let reply = invoke(
            script.path().to_str().expect("UTF-8 fixture path"),
            "one complete proposition",
            PROCESS_RUNTIME,
        )
        .expect("decisive judge completes");

        assert_eq!(reply, "YES\n");
    }

    #[test]
    fn ambiguous_process_exhausts_only_the_declared_framings() {
        let script = Script::new("cat >/dev/null; printf 'perhaps\\n'");
        let mut process = Process::with(
            script
                .path()
                .to_str()
                .expect("UTF-8 fixture path")
                .to_owned(),
            PROCESS_RUNTIME,
        );

        let error = process
            .adjudicate(&prompts())
            .expect_err("ambiguous process never supplies a verdict");

        assert_eq!(error.reason(), "ambiguous");
        assert!(error.detail().contains("3 attempts"));
    }

    #[test]
    fn timeout_and_output_overflow_are_bounded_failures() {
        let timeout = Script::new("cat >/dev/null; exec /usr/bin/tail -f /dev/null");
        let error = invoke(
            timeout.path().to_str().expect("UTF-8 fixture path"),
            "prompt",
            Duration::from_millis(40),
        )
        .expect_err("nonterminating judge times out");
        assert_eq!(error.reason(), "timeout");

        let overflow = Script::new(
            "cat >/dev/null; i=0; while [ $i -le 4096 ]; do printf x; i=$((i + 1)); done",
        );
        let error = invoke(
            overflow.path().to_str().expect("UTF-8 fixture path"),
            "prompt",
            PROCESS_RUNTIME,
        )
        .expect_err("oversized reply is rejected");
        assert_eq!(error.reason(), "ambiguous");
        assert!(error.detail().contains("exceeded 4096 bytes"));
    }

    #[test]
    fn inherited_descendant_pipe_is_killed_and_named() {
        let script = Script::new("cat >/dev/null; sleep 30 & printf 'YES\\n'");
        let started = std::time::Instant::now();

        let error = invoke(
            script.path().to_str().expect("UTF-8 fixture path"),
            "prompt",
            PROCESS_RUNTIME,
        )
        .expect_err("a retained descendant pipe is not a clean answer");

        assert_eq!(error.reason(), "judge_error");
        assert!(error.detail().contains("retained an output pipe"));
        assert!(started.elapsed() < PROCESS_RUNTIME);
    }

    #[test]
    fn parser_accepts_only_one_normalized_yes_or_no() {
        assert_eq!(parse_yes_no(" YES\n"), Some(true));
        assert_eq!(parse_yes_no("no."), Some(false));
        assert_eq!(parse_yes_no("Answer: NO"), None);
        assert_eq!(parse_yes_no("YES but NO"), None);
        assert_eq!(parse_yes_no("perhaps"), None);
    }
}
