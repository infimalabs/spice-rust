//! CLI acquisition and typed resident mappings for lane lifecycle commands.

use std::path::{Path, PathBuf};

use clap::ArgMatches;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, LaneAction, LaneRecall, LaneSeat, LaneState};
use wire::{CanonicalUuid, Fault, FaultClass, Outcome, Refusal};

use crate::cli::Contract;
use crate::command_client::{self, SelectedCommand};

/// Execute one of the five landed lane-family leaves.
pub(crate) fn run(matches: &ArgMatches, contract: &Contract) -> Outcome<CommandAnswer> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return environment_fault("lane::cwd-unavailable", &error.to_string()),
    };
    let repository = match Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => return environment_fault("lane::repository-unresolved", &error.to_string()),
    };

    match contract.path {
        ["lane"] => command_client::execute(&repository, contract, SelectedCommand::LaneRead),
        ["lane", "add"] => add(&repository, &current, matches, contract),
        ["lane", "drain"] => state(&repository, matches, contract, LaneAction::Drain),
        ["lane", "wake"] => state(&repository, matches, contract, LaneAction::Wake),
        ["recall"] => recall(&repository, matches, contract),
        _ => protocol_fault(
            "lane::dispatch-gap",
            "lane command runner received a contract outside its closed family",
        ),
    }
}

fn add(
    repository: &Repository,
    current: &Path,
    matches: &ArgMatches,
    contract: &Contract,
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
    let resolved = match spice::config::load_repository(&worktree, &[]) {
        Ok(resolved) => resolved,
        Err(error) => return environment_fault("lane::configuration-refused", &error.to_string()),
    };
    let life = matches.get_one::<String>("life").map_or_else(
        || resolved.root().lane().life().as_str().to_owned(),
        |life| life.to_ascii_lowercase(),
    );
    let vendor = match matches.get_one::<String>("vendor") {
        Some(vendor) => match vendor.parse::<wire::Name>() {
            Ok(vendor) => vendor,
            Err(error) => return protocol_fault("lane::vendor-invalid", &error.to_string()),
        },
        None => resolved.root().lane().vendor().clone(),
    };
    let body = LaneSeat {
        lane: new_uuid(),
        worktree: worktree.worktree_root().display().to_string(),
        life,
        vendor,
    };
    command_client::execute(
        repository,
        contract,
        SelectedCommand::LaneSeat { body: &body },
    )
}

fn state(
    repository: &Repository,
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
    let body = LaneState { lane, action };
    command_client::execute(
        repository,
        contract,
        SelectedCommand::LaneState { body: &body },
    )
}

fn recall(
    repository: &Repository,
    matches: &ArgMatches,
    contract: &Contract,
) -> Outcome<CommandAnswer> {
    let lane = match matches.get_one::<String>("LANE") {
        Some(lane) => match lane.parse::<CanonicalUuid>() {
            Ok(lane) => Some(lane),
            Err(error) => return protocol_fault("recall::lane-invalid", &error.to_string()),
        },
        None => None,
    };
    let body = LaneRecall { lane };
    command_client::execute(
        repository,
        contract,
        SelectedCommand::LaneRecall { body: &body },
    )
}

fn refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("worktree".to_owned()),
        expected,
        observed,
        remedy: remedy.to_owned(),
    })
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
