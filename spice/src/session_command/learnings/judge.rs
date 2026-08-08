//! Independent, bounded repository-general learning adjudication.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

use super::LearningCandidate;

const MAX_ATTEMPTS: usize = 2;
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REPLY_BYTES: u64 = 4 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const CRITERION: &str = "The candidate is a durable, repo-general operational fact worth carrying to successor agents. It is reusable across future work in this repository and is not task status, speculation, a secret, one-off implementation trivia, or a statement without reusable operational value.";

#[cfg(target_os = "macos")]
pub(super) const DEFAULT_JUDGE: &str = "afm-cli";
#[cfg(not(target_os = "macos"))]
pub(super) const DEFAULT_JUDGE: &str = "spice-judge";

pub(super) trait Judge {
    fn judge(&mut self, candidate: &LearningCandidate) -> Result<bool, JudgeError>;
}

#[derive(Debug)]
pub(super) struct JudgeError {
    reason: &'static str,
    detail: String,
}

impl JudgeError {
    pub(super) const fn reason(&self) -> &'static str {
        self.reason
    }

    pub(super) fn detail(&self) -> &str {
        &self.detail
    }
}

#[derive(Debug)]
pub(super) struct ProcessJudge {
    binary: String,
    timeout: Duration,
}

impl Default for ProcessJudge {
    fn default() -> Self {
        Self {
            binary: DEFAULT_JUDGE.to_owned(),
            timeout: ATTEMPT_TIMEOUT,
        }
    }
}

impl Judge for ProcessJudge {
    fn judge(&mut self, candidate: &LearningCandidate) -> Result<bool, JudgeError> {
        let prompt = prompt(candidate);
        let mut replies = Vec::new();
        for _ in 0..MAX_ATTEMPTS {
            let raw = invoke(&self.binary, &prompt, self.timeout)?;
            match parse_yes_no(&raw) {
                Some(answer) => return Ok(answer),
                None => replies.push(compact(&raw)),
            }
        }
        Err(JudgeError {
            reason: "ambiguous",
            detail: format!(
                "judge did not return one YES/NO in {MAX_ATTEMPTS} attempts: {}",
                replies.join(" | ")
            ),
        })
    }
}

fn prompt(candidate: &LearningCandidate) -> String {
    let statement = [
        ("statement", candidate.statement.as_str()),
        ("evidence", candidate.evidence.as_str()),
        ("source_task", candidate.source_task.as_str()),
        ("project_stem", candidate.project_stem.as_str()),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(name, value)| format!("{name}: {}", compact(value)))
    .collect::<Vec<_>>()
    .join("; ");
    format!(
        "IFF \"{statement}\" SATISFIES \"{CRITERION}\": ANSWER ONLY \"YES\".\n\
         IFF \"{statement}\" DOES NOT SATISFY \"{CRITERION}\": ANSWER ONLY \"NO\".\n"
    )
}

pub(super) fn invoke(binary: &str, prompt: &str, timeout: Duration) -> Result<String, JudgeError> {
    let mut command = Command::new(binary);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| JudgeError {
        reason: "unavailable",
        detail: format!("could not launch {binary}: {error}"),
    })?;
    let Some(mut input) = child.stdin.take() else {
        stop(&mut child);
        return Err(JudgeError {
            reason: "judge_error",
            detail: "judge stdin was unavailable".to_owned(),
        });
    };
    if let Err(error) = input.write_all(prompt.as_bytes()) {
        stop(&mut child);
        return Err(JudgeError {
            reason: "judge_error",
            detail: format!("could not write judge prompt: {error}"),
        });
    }
    drop(input);

    let Some(stdout) = child.stdout.take() else {
        stop(&mut child);
        return Err(JudgeError {
            reason: "judge_error",
            detail: "judge stdout was unavailable".to_owned(),
        });
    };
    let Some(stderr) = child.stderr.take() else {
        stop(&mut child);
        return Err(JudgeError {
            reason: "judge_error",
            detail: "judge stderr was unavailable".to_owned(),
        });
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
                return Err(JudgeError {
                    reason: "timeout",
                    detail: format!("judge exceeded {}ms", timeout.as_millis()),
                });
            }
            Err(error) => {
                stop(&mut child);
                return Err(JudgeError {
                    reason: "judge_error",
                    detail: format!("could not observe judge: {error}"),
                });
            }
        }
    };
    let stdout = match receive_reader(stdout_reader) {
        Ok(stdout) => stdout,
        Err(error) => {
            stop(&mut child);
            return Err(error);
        }
    };
    let stderr = match receive_reader(stderr_reader) {
        Ok(stderr) => stderr,
        Err(error) => {
            stop(&mut child);
            return Err(error);
        }
    };
    if !status.success() {
        return Err(JudgeError {
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
                }
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

fn receive_reader(reader: Receiver<std::io::Result<Vec<u8>>>) -> Result<String, JudgeError> {
    let bytes = reader
        .recv_timeout(Duration::from_millis(250))
        .map_err(|error| JudgeError {
            reason: "judge_error",
            detail: format!("judge output reader did not finish: {error}"),
        })?
        .map_err(|error| JudgeError {
            reason: "judge_error",
            detail: format!("judge output was unreadable: {error}"),
        })?;
    if bytes.len() as u64 > MAX_REPLY_BYTES {
        return Err(JudgeError {
            reason: "ambiguous",
            detail: format!("judge reply exceeded {MAX_REPLY_BYTES} bytes"),
        });
    }
    String::from_utf8(bytes).map_err(|error| JudgeError {
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

pub(super) fn parse_yes_no(raw: &str) -> Option<bool> {
    let mut tokens = raw
        .to_uppercase()
        .chars()
        .filter(|character| matches!(character, 'Y' | 'E' | 'S' | 'N' | 'O' | ' '))
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    tokens.sort();
    tokens.dedup();
    match tokens.as_slice() {
        [answer] if answer == "YES" => Some(true),
        [answer] if answer == "NO" => Some(false),
        _ => None,
    }
}

fn compact(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
pub(super) struct FunctionJudge<F>(pub(super) F);

#[cfg(test)]
impl<F> Judge for FunctionJudge<F>
where
    F: FnMut(&LearningCandidate) -> Result<bool, JudgeError>,
{
    fn judge(&mut self, candidate: &LearningCandidate) -> Result<bool, JudgeError> {
        (self.0)(candidate)
    }
}

#[cfg(test)]
pub(super) fn failure(reason: &'static str, detail: &str) -> JudgeError {
    JudgeError {
        reason,
        detail: detail.to_owned(),
    }
}
