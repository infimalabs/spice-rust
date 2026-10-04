//! CLI acquisition and typed resident mappings for lane lifecycle commands.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use spice::repository::Repository;
use wire::intent::{
    CommandAnswer, LaneAction, LaneAnswer, LaneEffortRetune, LaneFastRetune, LaneRecall,
    LaneRetune, LaneSeat, LaneState,
};
use wire::{CanonicalUuid, Fault, FaultClass, Outcome, Refusal, Reply};

use crate::cli::Contract;
use crate::command_client::{self, SelectedCommand};

mod list;
pub(crate) mod purge;

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
        Err(error) => {
            return environment_fault(spice::repository::ResolveError::REASON, &error.to_string());
        }
    };
    let state_paths = match repository.admit_state() {
        Ok(state_paths) => state_paths,
        Err(error) => return crate::state_failure(&error).outcome(),
    };
    let serve = state_paths.serve();

    match contract.path {
        ["lane"] => command_client::execute(serve, contract, SelectedCommand::LaneRead),
        ["lane", "list"] => list::run(&repository, matches, contract),
        ["lane", "add"] => add(
            &repository,
            serve,
            &current,
            matches,
            contract,
            configuration,
        ),
        ["lane", "drain"] => state(serve, matches, contract, LaneAction::Drain),
        ["lane", "wake"] => state(serve, matches, contract, LaneAction::Wake),
        ["lane", "model"] => model(serve, matches, contract),
        ["lane", "effort"] => effort(serve, matches, contract),
        ["lane", "fast"] => fast(serve, matches, contract),
        ["lane", "remove"] => state(serve, matches, contract, LaneAction::Remove),
        ["lane", "import"] => import(&repository, serve, &current, matches, contract),
        ["recall"] => recall(serve, matches, contract),
        _ => protocol_fault(
            "lane::dispatch-gap",
            "lane command runner received a contract outside its closed family",
        ),
    }
}

fn add(
    repository: &Repository,
    serve: &Path,
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
    let team = match matches.get_one::<String>("team") {
        Some(team) => match team.parse::<wire::CanonicalUuid>() {
            Ok(team) => team,
            Err(error) => return protocol_fault("lane::team-invalid", &error.to_string()),
        },
        None => lane,
    };
    let topology_revision = match observe_topology(serve) {
        Ok(revision) => revision,
        Err(outcome) => return outcome,
    };
    let body = lane_seat_body(
        matches,
        topology_revision,
        lane,
        team,
        worktree.worktree_root().display().to_string(),
        life,
        vendor,
    );
    command_client::execute(serve, contract, SelectedCommand::LaneSeat { body: &body })
}

fn lane_seat_body(
    matches: &ArgMatches,
    topology_revision: u64,
    lane: CanonicalUuid,
    team: CanonicalUuid,
    worktree: String,
    life: String,
    vendor: wire::Name,
) -> LaneSeat {
    LaneSeat {
        topology_revision,
        lane,
        team,
        name: matches
            .get_one::<String>("name")
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty()),
        worktree,
        life,
        vendor,
        model: matches
            .get_one::<String>("model")
            .map(|model| model.trim().to_owned())
            .filter(|model| !model.is_empty()),
        effort: matches.get_one::<String>("effort").cloned(),
        resume: None,
    }
}

fn observe_topology(serve: &Path) -> Result<u64, Outcome<CommandAnswer>> {
    read_lanes(serve).map(|reply| reply.body.topology_revision)
}

/// Read the resident's current lane table under the lane-read contract.
fn read_lanes<B>(serve: &Path) -> Result<Reply<Box<LaneAnswer>>, Outcome<B>> {
    let contract = crate::cli::contract(&["lane"])
        .expect("the generated CLI registry carries the lane-read contract");
    match command_client::execute(serve, contract, SelectedCommand::LaneRead) {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Lane(body),
        }) => Ok(Reply { position, body }),
        Outcome::Reply(_) => Err(protocol_fault(
            "lane::answer-invalid",
            "resident lane read returned a non-lane answer",
        )),
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
    serve: &Path,
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
    command_client::execute(serve, contract, SelectedCommand::LaneState { body: &body })
}

/// State the model spelling one seated lane's next launch runs.
///
/// Naming no spelling clears the seat, which is how a lane returns to the
/// vendor's configured model: the request states the seat outright rather than
/// describing a change to it, so absence is an instruction and not an omission.
fn model(serve: &Path, matches: &ArgMatches, contract: &Contract) -> Outcome<CommandAnswer> {
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
    let topology_revision = match observe_topology(serve) {
        Ok(revision) => revision,
        Err(outcome) => return outcome,
    };
    let body = LaneRetune {
        topology_revision,
        lane,
        model: matches
            .get_one::<String>("MODEL")
            .map(|model| model.trim().to_owned())
            .filter(|model| !model.is_empty()),
    };
    command_client::execute(serve, contract, SelectedCommand::LaneRetune { body: &body })
}
/// State the reasoning effort one seated lane's next launch runs.
fn effort(serve: &Path, matches: &ArgMatches, contract: &Contract) -> Outcome<CommandAnswer> {
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
    let topology_revision = match observe_topology(serve) {
        Ok(revision) => revision,
        Err(outcome) => return outcome,
    };
    let effort = matches
        .get_one::<String>("EFFORT")
        .map(|effort| effort.trim().to_owned())
        .filter(|effort| !effort.is_empty());
    if effort
        .as_deref()
        .is_some_and(|effort| effort.parse::<vendor::Effort>().is_err())
    {
        return protocol_fault(
            "lane::effort-invalid",
            "effort must be low, medium, high, xhigh, or max",
        );
    }
    let body = LaneEffortRetune {
        topology_revision,
        lane,
        effort,
    };
    command_client::execute(
        serve,
        contract,
        SelectedCommand::LaneEffortRetune { body: &body },
    )
}

/// State whether one seated lane's next launch runs in fast mode; omission
/// returns the seat to the global setting.
fn fast(serve: &Path, matches: &ArgMatches, contract: &Contract) -> Outcome<CommandAnswer> {
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
    let topology_revision = match observe_topology(serve) {
        Ok(revision) => revision,
        Err(outcome) => return outcome,
    };
    let body = LaneFastRetune {
        topology_revision,
        lane,
        fast_mode: matches.get_one::<String>("on|off").map(|mode| mode == "on"),
    };
    command_client::execute(
        serve,
        contract,
        SelectedCommand::LaneFastRetune { body: &body },
    )
}

fn import(
    repository: &Repository,
    serve: &Path,
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
    command_client::execute(serve, contract, SelectedCommand::LaneState { body: &body })
}

fn recall(serve: &Path, matches: &ArgMatches, contract: &Contract) -> Outcome<CommandAnswer> {
    let lane = match matches.get_one::<String>("LANE") {
        Some(lane) => match lane.parse::<CanonicalUuid>() {
            Ok(lane) => Some(lane),
            Err(error) => return protocol_fault("recall::lane-invalid", &error.to_string()),
        },
        None => None,
    };
    let body = LaneRecall { lane };
    command_client::execute(serve, contract, SelectedCommand::LaneRecall { body: &body })
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

fn protocol_fault<B>(reason: &str, detail: &str) -> Outcome<B> {
    Outcome::Fault(Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn environment_fault<B>(reason: &str, detail: &str) -> Outcome<B> {
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
            .try_get_matches_from([
                "spice",
                "lane",
                "add",
                "--name",
                " reviewer ",
                "--model",
                " gpt-5.5 ",
                "--effort",
                "low",
            ])
            .expect("lane add parses without resume");
        let (_, lane_matches) = matches.subcommand().expect("lane subcommand exists");
        let (_, add_matches) = lane_matches
            .subcommand()
            .expect("lane add subcommand exists");
        let lane = CanonicalUuid::new(uuid::Uuid::from_u128(1));
        let team = CanonicalUuid::new(uuid::Uuid::from_u128(2));
        let body = lane_seat_body(
            add_matches,
            7,
            lane,
            team,
            "/tmp/spice-lane".to_owned(),
            "drive".to_owned(),
            "omp".parse().expect("fixture vendor parses"),
        );

        assert_eq!(body.topology_revision, 7);
        assert_eq!(body.lane, lane);
        assert_eq!(body.team, team);
        assert_eq!(body.name.as_deref(), Some("reviewer"));
        assert_eq!(body.worktree, "/tmp/spice-lane");
        assert_eq!(body.life, "drive");
        assert_eq!(body.vendor.as_str(), "omp");
        assert_eq!(body.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(body.effort.as_deref(), Some("low"));
        assert_eq!(body.resume, None);
    }

    #[test]
    fn lane_add_rejects_an_effort_outside_the_closed_vocabulary() {
        let error = crate::cli::command()
            .try_get_matches_from(["spice", "lane", "add", "--effort", "ultra"])
            .expect_err("an unknown effort is refused by the generated parser");
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidValue);
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
