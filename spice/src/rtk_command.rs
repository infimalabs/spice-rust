//! Daemonless RTK history resolution and query by agent identity.

use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;

use clap::ArgMatches;
use wire::{
    CanonicalUuid, Fault, FaultClass, Outcome, Refusal, RefusalDiagnostic, Reply, ReplyPosition,
    RtkGainReport,
};

use spice::repository::{Repository, STATE_DIRECTORY};

/// Resolve one agent database and run the configured RTK gain query against it.
pub(crate) fn gain(matches: &ArgMatches) -> Outcome<RtkGainReport> {
    let requested = matches
        .get_one::<String>("AGENT_UUID")
        .expect("clap requires the registered agent UUID operand");
    let identity = match AgentIdentity::parse(requested) {
        Some(identity) => identity,
        None => {
            return Failure::refusal(
                "rtk::agent-invalid",
                format!(
                    "agent UUID {requested:?} is not canonical or compact lower-case UUID text"
                ),
            )
            .outcome();
        }
    };
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => {
            return Failure::fault(
                FaultClass::Environment,
                "rtk::working-directory-unavailable",
                error.to_string(),
            )
            .outcome();
        }
    };
    let repository = match Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => {
            return Failure::fault(
                FaultClass::Environment,
                "rtk::repository-unresolved",
                error.to_string(),
            )
            .outcome();
        }
    };
    let resolved = match spice::config::load_repository(&repository, &[]) {
        Ok(resolved) => resolved,
        Err(error) => {
            return Failure::refusal(
                "rtk::configuration-refused",
                format!("cannot resolve the configured RTK executable: {error}"),
            )
            .outcome();
        }
    };
    let database = match resolve_history(&repository, &identity) {
        Ok(database) => database,
        Err(failure) => return failure.outcome(),
    };
    let database_text = match database.to_str() {
        Some(database) => database.to_owned(),
        None => {
            return Failure::fault(
                FaultClass::Store,
                "rtk::history-path-invalid",
                format!("RTK database path is not UTF-8: {database:?}"),
            )
            .outcome();
        }
    };
    let executable = resolved.root().rtk().executable();
    let output = match Command::new(executable)
        .arg("gain")
        .current_dir(repository.worktree_root())
        .env(spice::rtk::HISTORY_VARIABLE, &database)
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            return Failure::fault(
                FaultClass::Environment,
                "rtk::command-unavailable",
                format!(
                    "configured RTK executable {executable:?} could not query {database_text}: {error}"
                ),
            )
            .outcome();
        }
    };
    if !output.status.success() {
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        return Failure::fault(
            FaultClass::Environment,
            "rtk::command-failed",
            format!(
                "configured RTK executable {executable:?} failed for {database_text} with {}; stderr: {}",
                output.status,
                diagnostic.trim_end()
            ),
        )
        .outcome();
    }
    let output = match String::from_utf8(output.stdout) {
        Ok(output) => output,
        Err(error) => {
            return Failure::fault(
                FaultClass::Protocol,
                "rtk::output-invalid",
                format!("rtk gain output for {database_text} is not UTF-8: {error}"),
            )
            .outcome();
        }
    };

    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: RtkGainReport {
            agent: requested.clone(),
            database: database_text,
            output,
        },
    })
}

#[derive(Debug)]
struct AgentIdentity {
    canonical: String,
}

impl AgentIdentity {
    fn parse(text: &str) -> Option<Self> {
        let uuid = text
            .parse::<CanonicalUuid>()
            .map(CanonicalUuid::get)
            .ok()
            .or_else(|| {
                (text.len() == 32
                    && text
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
                .then(|| uuid::Uuid::parse_str(text).ok())
                .flatten()
            })?;
        Some(Self {
            canonical: uuid.as_hyphenated().to_string(),
        })
    }
}

#[derive(Debug)]
struct HistoryObservation {
    path: PathBuf,
    problem: Option<String>,
}

impl HistoryObservation {
    fn describe(&self) -> String {
        self.problem.as_ref().map_or_else(
            || format!("{} (readable)", self.path.display()),
            |problem| format!("{} ({problem})", self.path.display()),
        )
    }
}

fn resolve_history(repository: &Repository, identity: &AgentIdentity) -> Result<PathBuf, Failure> {
    let current = repository
        .common_dir()
        .join(STATE_DIRECTORY)
        .join("agents")
        .join(&identity.canonical)
        .join("rtk/history.db");
    if let Some(observed) = observe_history(current) {
        return match observed.problem {
            None => Ok(observed.path),
            Some(_) => Err(Failure::fault(
                FaultClass::Store,
                "rtk::history-unreadable",
                format!("agent history is unavailable at {}", observed.describe()),
            )),
        };
    }

    Err(Failure::refusal(
        "rtk::history-absent",
        format!(
            "no current Rust RTK history database was found for agent {} at {}",
            identity.canonical,
            repository
                .common_dir()
                .join(STATE_DIRECTORY)
                .join("agents")
                .join(&identity.canonical)
                .join("rtk/history.db")
                .display()
        ),
    ))
}

fn observe_history(path: PathBuf) -> Option<HistoryObservation> {
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => {
            return Some(HistoryObservation {
                path,
                problem: Some(format!("metadata unavailable: {error}")),
            });
        }
    };
    let problem = if metadata.file_type().is_symlink() {
        Some("symbolic links are not admitted".to_owned())
    } else if !metadata.is_file() {
        Some("not a regular file".to_owned())
    } else {
        File::open(&path)
            .err()
            .map(|error| format!("unreadable: {error}"))
    };
    Some(HistoryObservation { path, problem })
}

enum Failure {
    Refusal(Refusal),
    Fault(Fault),
}

impl Failure {
    fn refusal(reason: &str, detail: String) -> Self {
        Self::Refusal(Refusal::bare(
            RefusalDiagnostic::new(reason.to_owned(), Some("agent-uuid".to_owned()), None, None)
                .with_detail(detail),
        ))
    }

    fn fault(class: FaultClass, reason: &str, detail: String) -> Self {
        Self::Fault(Fault {
            class,
            reason: reason.to_owned(),
            detail,
        })
    }

    fn outcome(self) -> Outcome<RtkGainReport> {
        match self {
            Self::Refusal(refusal) => Outcome::Refusal(refusal),
            Self::Fault(fault) => Outcome::Fault(fault),
        }
    }
}
