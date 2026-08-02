use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use wait_timeout::ChildExt as _;

fn required_read(
    root: &Path,
    args: &[&str],
    action: &str,
) -> Result<String, Error> {
    let output = run(root, args)?;
    if output.success() {
        Ok(output.stdout.trim().to_owned())
    } else {
        Err(Error::fault(
            "git::command-failed",
            output.detail(action),
        ))
    }
}

fn try_read(
    root: &Path,
    args: &[&str],
) -> Result<Option<String>, Error> {
    let output = run(root, args)?;
    Ok(output.success().then(|| output.stdout.trim().to_owned()))
}

fn run(root: &Path, args: &[&str]) -> Result<GitOutput, Error> {
    run_bounded(root, args, None)
}

fn run_with_input(
    root: &Path,
    args: &[&str],
    input: &[u8],
) -> Result<GitOutput, Error> {
    run_bounded(root, args, Some(input))
}

fn run_bounded(
    root: &Path,
    args: &[&str],
    input: Option<&[u8]>,
) -> Result<GitOutput, Error> {
    let mut child = spawn_git(root, args, input.is_some())?;
    let readers = match CommandReaders::take(&mut child) {
        Ok(readers) => readers,
        Err(error) => {
            stop_git(&mut child);
            return Err(error);
        }
    };
    if let Err(error) = write_git_input(&mut child, input) {
        stop_git(&mut child);
        readers.discard();
        return Err(error);
    }
    let status = wait_for_git(&mut child, args);
    let output = readers.finish(args);
    let status = status?;
    let (stdout, stderr) = output?;
    Ok(GitOutput {
        status: status.code().unwrap_or(-1),
        stdout,
        stderr,
    })
}

fn spawn_git(
    root: &Path,
    args: &[&str],
    piped_input: bool,
) -> Result<std::process::Child, Error> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=5",
        )
        .stdin(if piped_input {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            Error::fault("git::launch-failed", error.to_string())
        })
}

fn write_git_input(
    child: &mut std::process::Child,
    input: Option<&[u8]>,
) -> Result<(), Error> {
    let Some(input) = input else {
        return Ok(());
    };
    child
        .stdin
        .take()
        .ok_or_else(|| {
            Error::fault(
                "git::stdin-missing",
                "Git stdin pipe was unavailable",
            )
        })?
        .write_all(input)
        .map_err(|error| {
            Error::fault("git::stdin-failed", error.to_string())
        })
}

fn wait_for_git(
    child: &mut std::process::Child,
    args: &[&str],
) -> Result<std::process::ExitStatus, Error> {
    let network = args
        .first()
        .is_some_and(|name| matches!(*name, "fetch" | "push"));
    let limit = if network {
        NETWORK_COMMAND_LIMIT
    } else {
        LOCAL_COMMAND_LIMIT
    };
    match child.wait_timeout(limit) {
        Ok(Some(status)) => Ok(status),
        Ok(None) => {
            stop_git(child);
            Err(Error::fault(
                "git::command-timeout",
                format!(
                    "git {} exceeded {} seconds",
                    args.join(" "),
                    limit.as_secs(),
                ),
            ))
        }
        Err(error) => {
            stop_git(child);
            Err(Error::fault(
                "git::wait-failed",
                error.to_string(),
            ))
        }
    }
}

fn stop_git(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

struct CommandReaders {
    stdout: JoinHandle<Result<Vec<u8>, OutputReadError>>,
    stderr: JoinHandle<Result<Vec<u8>, OutputReadError>>,
}

impl CommandReaders {
    fn take(child: &mut std::process::Child) -> Result<Self, Error> {
        let stdout = child.stdout.take().ok_or_else(|| {
            Error::fault(
                "git::stdout-missing",
                "Git stdout pipe was unavailable",
            )
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            Error::fault(
                "git::stderr-missing",
                "Git stderr pipe was unavailable",
            )
        })?;
        let total = Arc::new(AtomicUsize::new(0));
        let stdout_total = Arc::clone(&total);
        let stdout = std::thread::spawn(move || {
            read_bounded(stdout, &stdout_total)
        });
        let stderr = std::thread::spawn(move || {
            read_bounded(stderr, &total)
        });
        Ok(Self { stdout, stderr })
    }

    fn finish(
        self,
        args: &[&str],
    ) -> Result<(String, String), Error> {
        let stdout = join_output(self.stdout, "stdout", args);
        let stderr = join_output(self.stderr, "stderr", args);
        Ok((stdout?, stderr?))
    }

    fn discard(self) {
        let _ = self.stdout.join();
        let _ = self.stderr.join();
    }
}

enum OutputReadError {
    Limit,
    Read(std::io::Error),
}

fn read_bounded(
    mut input: impl std::io::Read,
    total: &AtomicUsize,
) -> Result<Vec<u8>, OutputReadError> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(OutputReadError::Read)?;
        if count == 0 {
            return Ok(output);
        }
        let previous = total.fetch_add(count, Ordering::AcqRel);
        if previous.saturating_add(count) > OUTPUT_LIMIT {
            return Err(OutputReadError::Limit);
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

fn join_output(
    reader: JoinHandle<Result<Vec<u8>, OutputReadError>>,
    name: &str,
    args: &[&str],
) -> Result<String, Error> {
    let bytes = match reader.join() {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(OutputReadError::Limit)) => {
            return Err(Error::fault(
                "git::output-limit",
                format!(
                    "git {} exceeded {} bytes",
                    args.join(" "),
                    OUTPUT_LIMIT,
                ),
            ));
        }
        Ok(Err(OutputReadError::Read(error))) => {
            return Err(Error::fault(
                "git::output-read-failed",
                format!("Git {name}: {error}"),
            ));
        }
        Err(_) => {
            return Err(Error::fault(
                "git::output-reader-panicked",
                format!("Git {name} reader panicked"),
            ));
        }
    };
    String::from_utf8(bytes).map_err(|error| {
        Error::fault(
            "git::output-not-utf8",
            format!("Git {name}: {error}"),
        )
    })
}
