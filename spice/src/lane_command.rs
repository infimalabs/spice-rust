//! CLI acquisition and typed resident mappings for lane lifecycle commands.

use std::path::{Path, PathBuf};

use clap::ArgMatches;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, LaneAction, LaneRecall, LaneSeat, LaneState};
use wire::{CanonicalUuid, Fault, FaultClass, Outcome, Refusal};

use crate::cli::Contract;
use crate::command_client::{self, SelectedCommand};

/// Execute one landed lane-family leaf.
pub(crate) fn run(matches: &ArgMatches, contract: &Contract) -> Outcome<CommandAnswer> {
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
        ["lane", "add"] => add(&repository, canvas, &current, matches, contract),
        ["lane", "drain"] => state(canvas, matches, contract, LaneAction::Drain),
        ["lane", "wake"] => state(canvas, matches, contract, LaneAction::Wake),
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
        Err(error) => return configuration_refusal(&error),
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
        name: matches
            .get_one::<String>("name")
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty()),
        worktree: worktree.worktree_root().display().to_string(),
        life,
        vendor,
    };
    command_client::execute(canvas, contract, SelectedCommand::LaneSeat { body: &body })
}

fn configuration_refusal(error: &spice::config::LoadError) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: "lane::configuration-refused".to_owned(),
        witness: Some("effective-configuration".to_owned()),
        expected: Some(
            "tracked and local inputs admitted by the configuration registry".to_owned(),
        ),
        observed: Some(error.diagnostic()),
        remedy: "correct the named configuration inputs before seating the lane".to_owned(),
    })
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
