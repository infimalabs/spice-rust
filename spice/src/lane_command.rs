//! CLI acquisition and typed resident mappings for lane lifecycle commands.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, LaneAction, LaneRecall, LaneSeat, LaneState};
use wire::{CanonicalUuid, Fault, FaultClass, Outcome, Refusal};

use crate::cli::Contract;
use crate::command_client::{self, SelectedCommand};

/// Execute one landed lane-family leaf.
pub(crate) fn run(
    matches: &ArgMatches,
    contract: &Contract,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Outcome<CommandAnswer> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return environment_fault("lane::cwd-unavailable", &error.to_string()),
    };
    let repository = match Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => return environment_fault("lane::repository-unresolved", &error.to_string()),
    };
    let state_paths = match repository.admit_state() {
        Ok(state_paths) => state_paths,
        Err(error) => return crate::state_failure(&error).outcome(),
    };
    let canvas = state_paths.canvas();

    match contract.path {
        ["lane"] => command_client::execute(canvas, contract, SelectedCommand::LaneRead),
        ["lane", "add"] => add(
            &repository,
            canvas,
            &current,
            matches,
            contract,
            configuration,
        ),
        ["lane", "drain"] => state(canvas, matches, contract, LaneAction::Drain),
        ["lane", "wake"] => state(canvas, matches, contract, LaneAction::Wake),
        ["lane", "remove"] => state(canvas, matches, contract, LaneAction::Remove),
        ["lane", "import"] => import(&repository, canvas, &current, matches, contract),
        ["recall"] => recall(canvas, matches, contract),
        _ => protocol_fault(
            "lane::dispatch-gap",
            "lane command runner received a contract outside its closed family",
        ),
    }
}

fn add(
    repository: &Repository,
    canvas: &Path,
    current: &Path,
    matches: &ArgMatches,
    contract: &Contract,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Outcome<CommandAnswer> {
    let requested = matches
        .get_one::<String>("WORKTREE")
        .map_or_else(|| repository.worktree_root().to_path_buf(), PathBuf::from);
    let requested = if requested.is_absolute() {
        requested
    } else {
        current.join(requested)
    };
    let worktree = match Repository::discover(&requested) {
        Ok(worktree) => worktree,
        Err(error) => return environment_fault("lane::worktree-unresolved", &error.to_string()),
    };
    if worktree.common_dir() != repository.common_dir() {
        return refusal(
            "lane::foreign-worktree",
            Some(repository.common_dir().display().to_string()),
            Some(worktree.common_dir().display().to_string()),
            "choose an existing worktree belonging to the current Git common directory",
        );
    }
    let requested = match std::fs::canonicalize(&requested) {
        Ok(requested) => requested,
        Err(error) => return environment_fault("lane::worktree-unresolved", &error.to_string()),
    };
    if requested != worktree.worktree_root() {
        return refusal(
            "lane::worktree-not-root",
            Some(worktree.worktree_root().display().to_string()),
            Some(requested.display().to_string()),
            "name the root of one existing worktree, not a path nested beneath it",
        );
    }
    let loaded = match spice::config::capture_repository_scoped(&worktree, &[], configuration) {
        Ok(loaded) => loaded,
        Err(error) => return configuration_refusal(&error),
    };
    let resolved = loaded.resolved();
    let life = matches.get_one::<String>("life").map_or_else(
        || resolved.root().lane().life().as_str().to_owned(),
        |life| life.to_ascii_lowercase(),
    );
    let vendor = match resolve_vendor(
        matches.get_one::<String>("vendor").map(String::as_str),
        crate::bootstrap_environment("SPICE_AGENT_VENDOR"), // env-policy: allow
        resolved.root().lane().vendor(),
    ) {
        Ok(vendor) => vendor,
        Err(VendorResolutionError::EnvironmentEncoding) => {
            return protocol_fault(
                "lane::vendor-environment-invalid",
                "SPICE_AGENT_VENDOR must be valid UTF-8",
            );
        }
        Err(VendorResolutionError::Invalid(detail)) => {
            return protocol_fault("lane::vendor-invalid", &detail);
        }
    };
    let lane = new_uuid();
    let lineage_id = match matches.get_one::<String>("lineage") {
        Some(lineage) => match lineage.parse::<wire::CanonicalUuid>() {
            Ok(lineage_id) => lineage_id,
            Err(error) => return protocol_fault("lane::lineage-invalid", &error.to_string()),
        },
        None => lane,
    };
    let topology_revision = match observe_topology(canvas) {
        Ok(revision) => revision,
        Err(outcome) => return outcome,
    };
    let body = lane_seat_body(
        matches,
        topology_revision,
        lane,
        lineage_id,
        worktree.worktree_root().display().to_string(),
        life,
        vendor,
    );
    command_client::execute(canvas, contract, SelectedCommand::LaneSeat { body: &body })
}

fn lane_seat_body(
    matches: &ArgMatches,
    topology_revision: u64,
    lane: CanonicalUuid,
    lineage_id: CanonicalUuid,
    worktree: String,
    life: String,
    vendor: wire::Name,
) -> LaneSeat {
    LaneSeat {
        topology_revision,
        lane,
        lineage_id,
        name: matches
            .get_one::<String>("name")
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty()),
        worktree,
        life,
        vendor,
        resume: None,
    }
}

fn observe_topology(canvas: &Path) -> Result<u64, Outcome<CommandAnswer>> {
    let contract = crate::cli::contract(&["lane"])
        .expect("the generated CLI registry carries the lane-read contract");
    match command_client::execute(canvas, contract, SelectedCommand::LaneRead) {
        Outcome::Reply(reply) => match reply.body {
            CommandAnswer::Lane(answer) => Ok(answer.topology_revision),
            _ => Err(protocol_fault(
                "lane::answer-invalid",
                "resident lane read returned a non-lane answer",
            )),
        },
        Outcome::Refusal(refusal) => Err(Outcome::Refusal(refusal)),
        Outcome::Fault(fault) => Err(Outcome::Fault(fault)),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum VendorResolutionError {
    EnvironmentEncoding,
    Invalid(String),
}

fn resolve_vendor(
    explicit: Option<&str>,
    environment: Option<OsString>,
    configured: &wire::Name,
) -> Result<wire::Name, VendorResolutionError> {
    if let Some(explicit) = explicit {
        return explicit
            .parse()
            .map_err(|error: wire::NameError| VendorResolutionError::Invalid(error.to_string()));
    }
    let environment = environment
        .map(OsString::into_string)
        .transpose()
        .map_err(|_| VendorResolutionError::EnvironmentEncoding)?
        .filter(|value| !value.trim().is_empty());
    environment.map_or_else(
        || Ok(configured.clone()),
        |value| {
            value
                .trim()
                .to_ascii_lowercase()
                .parse()
                .map_err(|error: wire::NameError| VendorResolutionError::Invalid(error.to_string()))
        },
    )
}

fn configuration_refusal(error: &spice::config::LoadError) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
        "lane::configuration-refused".to_owned(),
        Some("effective-configuration".to_owned()),
        Some("tracked and local inputs admitted by the configuration registry".to_owned()),
        Some(error.diagnostic()),
    )))
}

fn state(
    canvas: &Path,
    matches: &ArgMatches,
    contract: &Contract,
    action: LaneAction,
) -> Outcome<CommandAnswer> {
    let Some(id) = matches.get_one::<String>("ID") else {
        return protocol_fault(
            "lane::id-missing",
            "generated parser omitted required lane ID",
        );
    };
    let lane = match id.parse::<CanonicalUuid>() {
        Ok(lane) => lane,
        Err(error) => return protocol_fault("lane::id-invalid", &error.to_string()),
    };
    let body = LaneState {
        lane,
        action,
        worktree: None,
    };
    command_client::execute(canvas, contract, SelectedCommand::LaneState { body: &body })
}

fn import(
    repository: &Repository,
    canvas: &Path,
    current: &Path,
    matches: &ArgMatches,
    contract: &Contract,
) -> Outcome<CommandAnswer> {
    let Some(id) = matches.get_one::<String>("ID") else {
        return protocol_fault(
            "lane::id-missing",
            "generated parser omitted required lane ID",
        );
    };
    let lane = match id.parse::<CanonicalUuid>() {
        Ok(lane) => lane,
        Err(error) => return protocol_fault("lane::id-invalid", &error.to_string()),
    };
    let requested = PathBuf::from(
        matches
            .get_one::<String>("WORKTREE")
            .expect("generated syntax requires the import worktree"),
    );
    let requested = if requested.is_absolute() {
        requested
    } else {
        current.join(requested)
    };
    let target = match Repository::discover(&requested) {
        Ok(target) => target,
        Err(error) => return environment_fault("lane::worktree-unresolved", &error.to_string()),
    };
    if target.common_dir() != repository.common_dir() {
        return refusal(
            "lane::foreign-worktree",
            Some(repository.common_dir().display().to_string()),
            Some(target.common_dir().display().to_string()),
            "choose an existing worktree belonging to the current Git common directory",
        );
    }
    let requested = match std::fs::canonicalize(requested) {
        Ok(requested) => requested,
        Err(error) => return environment_fault("lane::worktree-unresolved", &error.to_string()),
    };
    if requested != target.worktree_root() {
        return refusal(
            "lane::worktree-not-root",
            Some(target.worktree_root().display().to_string()),
            Some(requested.display().to_string()),
            "name the root of one existing worktree",
        );
    }
    let body = LaneState {
        lane,
        action: LaneAction::Wake,
        worktree: Some(requested.display().to_string()),
    };
    command_client::execute(canvas, contract, SelectedCommand::LaneState { body: &body })
}

fn recall(canvas: &Path, matches: &ArgMatches, contract: &Contract) -> Outcome<CommandAnswer> {
    let lane = match matches.get_one::<String>("LANE") {
        Some(lane) => match lane.parse::<CanonicalUuid>() {
            Ok(lane) => Some(lane),
            Err(error) => return protocol_fault("recall::lane-invalid", &error.to_string()),
        },
        None => None,
    };
    let body = LaneRecall { lane };
    command_client::execute(
        canvas,
        contract,
        SelectedCommand::LaneRecall { body: &body },
    )
}

fn refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    detail: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("worktree".to_owned()),
            expected,
            observed,
        )
        .with_detail(detail),
    ))
}

fn protocol_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn new_uuid() -> CanonicalUuid {
    CanonicalUuid::new(uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lane_add_body_does_not_read_removed_resume_argument() {
        let matches = crate::cli::command()
            .try_get_matches_from(["spice", "lane", "add", "--name", " reviewer "])
            .expect("lane add parses without resume");
        let (_, lane_matches) = matches.subcommand().expect("lane subcommand exists");
        let (_, add_matches) = lane_matches
            .subcommand()
            .expect("lane add subcommand exists");
        let lane = CanonicalUuid::new(uuid::Uuid::from_u128(1));
        let lineage = CanonicalUuid::new(uuid::Uuid::from_u128(2));
        let body = lane_seat_body(
            add_matches,
            7,
            lane,
            lineage,
            "/tmp/spice-lane".to_owned(),
            "drive".to_owned(),
            "codex".parse().expect("fixture vendor parses"),
        );

        assert_eq!(body.topology_revision, 7);
        assert_eq!(body.lane, lane);
        assert_eq!(body.lineage_id, lineage);
        assert_eq!(body.name.as_deref(), Some("reviewer"));
        assert_eq!(body.worktree, "/tmp/spice-lane");
        assert_eq!(body.life, "drive");
        assert_eq!(body.vendor.as_str(), "codex");
        assert_eq!(body.resume, None);
    }

    #[test]
    fn vendor_resolution_is_explicit_then_environment_then_configured_default() {
        let configured = "configured".parse().expect("configured vendor");
        assert_eq!(
            resolve_vendor(
                Some("explicit"),
                Some(OsString::from("environment")),
                &configured,
            )
            .expect("explicit vendor resolves")
            .as_str(),
            "explicit"
        );
        assert_eq!(
            resolve_vendor(None, Some(OsString::from(" EnViRoNmEnT ")), &configured)
                .expect("environment vendor resolves")
                .as_str(),
            "environment"
        );
        assert_eq!(
            resolve_vendor(None, Some(OsString::new()), &configured)
                .expect("configured vendor resolves")
                .as_str(),
            "configured"
        );
    }
}
