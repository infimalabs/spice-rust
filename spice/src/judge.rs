//! One bounded external YES/NO adjudication process shared by every question.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REPLY_BYTES: u64 = 4 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

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
    pub(crate) const fn reason(&self) -> &'static str {
        self.reason
    }

    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
}

/// The sole executable/model boundary used by adjudication consumers.
#[derive(Debug)]
pub(crate) struct Process {
    binary: String,
    timeout: Duration,
}

impl Default for Process {
    fn default() -> Self {
        Self {
            binary: crate::cli::JUDGE_EXECUTABLE.to_owned(),
            timeout: ATTEMPT_TIMEOUT,
        }
    }
}

impl Process {
    /// Ask ordered equivalent framings until exactly one normalized answer lands.
    pub(crate) fn adjudicate(&mut self, prompts: &[Prompt]) -> Result<Decision, Error> {
        let binary = self.binary.clone();
        let timeout = self.timeout;
        adjudicate_with(prompts, |prompt| invoke(&binary, prompt, timeout))
    }

    #[cfg(test)]
    pub(crate) fn with(binary: String, timeout: Duration) -> Self {
        Self { binary, timeout }
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

pub(crate) fn invoke(binary: &str, prompt: &str, timeout: Duration) -> Result<String, Error> {
    let mut command = Command::new(binary);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| Error {
        reason: "unavailable",
        detail: format!("could not launch {binary}: {error}"),
    })?;
    let Some(mut input) = child.stdin.take() else {
        stop(&mut child);
        return Err(failure_error("judge_error", "judge stdin was unavailable"));
    };
    if let Err(error) = input.write_all(prompt.as_bytes()) {
        stop(&mut child);
        return Err(Error {
            reason: "judge_error",
            detail: format!("could not write judge prompt: {error}"),
        });
    }
    drop(input);

    let Some(stdout) = child.stdout.take() else {
        stop(&mut child);
        return Err(failure_error("judge_error", "judge stdout was unavailable"));
    };
    let Some(stderr) = child.stderr.take() else {
        stop(&mut child);
        return Err(failure_error("judge_error", "judge stderr was unavailable"));
    };
    let stdout_reader = reader(stdout);
    let stderr_reader = reader(stderr);

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                stop(&mut child);
                return Err(Error {
                    reason: "timeout",
                    detail: format!("judge exceeded {}ms", timeout.as_millis()),
                });
            }
            Err(error) => {
                stop(&mut child);
                return Err(Error {
                    reason: "judge_error",
                    detail: format!("could not observe judge: {error}"),
                });
            }
        }
    };
    let stdout = receive_reader(stdout_reader)?;
    let stderr = receive_reader(stderr_reader)?;
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

fn bounded_read(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_REPLY_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn reader(input: impl Read + Send + 'static) -> Receiver<std::io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(bounded_read(input));
    });
    receiver
}

fn receive_reader(reader: Receiver<std::io::Result<Vec<u8>>>) -> Result<String, Error> {
    let bytes = reader
        .recv_timeout(Duration::from_millis(250))
        .map_err(|error| Error {
            reason: "judge_error",
            detail: format!("judge output reader did not finish: {error}"),
        })?
        .map_err(|error| Error {
            reason: "judge_error",
            detail: format!("judge output was unreadable: {error}"),
        })?;
    if bytes.len() as u64 > MAX_REPLY_BYTES {
        return Err(Error {
            reason: "ambiguous",
            detail: format!("judge reply exceeded {MAX_REPLY_BYTES} bytes"),
        });
    }
    String::from_utf8(bytes).map_err(|error| Error {
        reason: "ambiguous",
        detail: format!("judge reply was not UTF-8: {error}"),
    })
}

fn stop(child: &mut std::process::Child) {
    #[cfg(unix)]
    let group_signaled = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|group| {
            matches!(
                rustix::process::kill_process_group(group, rustix::process::Signal::KILL),
                Ok(()) | Err(rustix::io::Errno::SRCH)
            )
        });
    #[cfg(not(unix))]
    let group_signaled = false;
    if !group_signaled {
        let _ = child.kill();
    }
    let _ = child.wait();
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
    fn parser_accepts_only_one_normalized_yes_or_no() {
        assert_eq!(parse_yes_no(" YES\n"), Some(true));
        assert_eq!(parse_yes_no("no."), Some(false));
        assert_eq!(parse_yes_no("Answer: NO"), None);
        assert_eq!(parse_yes_no("YES but NO"), None);
        assert_eq!(parse_yes_no("perhaps"), None);
    }
}
