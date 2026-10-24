//! Bounded, relevance-ordered lane list projection.

use std::collections::BTreeMap;
use std::fmt;

use clap::ArgMatches;
use spice::ambient::Ambient;
use spice::repository::Repository;
use wire::intent::{BoardAnswer, BoardRead, CommandAnswer};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, CommandKind, Fault, FaultClass, Outcome, Refusal,
    Reply, SeatAddress,
};

use crate::cli::Contract;
use crate::command_client;

pub(super) fn run(
    repository: &Repository,
    matches: &ArgMatches,
    contract: &Contract,
    ambient: &Ambient,
) -> Outcome<LanePage> {
    let options = match LaneListOptions::from_matches(matches) {
        Ok(options) => options,
        Err(outcome) => return outcome,
    };
    match command_client::execute_typed(
        command_client::Host {
            repository,
            ambient,
        },
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        // Active claims sit only on open tasks; terminal history is not read.
        &BoardRead::Open,
    ) {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Board(board),
        }) => match lane_list(&board, &options) {
            Ok(page) => Outcome::Reply(Reply {
                position,
                body: page,
            }),
            Err(outcome) => outcome,
        },
        Outcome::Reply(_) => protocol_fault(
            "lane::list-answer-invalid",
            "lane list board read returned another answer shape",
        ),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

/// One bounded page of lanes, each with the peer address it offers.
///
/// Text form speaks one line per lane, or names the command that seats the
/// first lane; `--json` keeps only the typed rows.
#[derive(Debug, serde::Serialize)]
#[serde(transparent)]
pub(crate) struct LanePage(wire::output::Bounded<LaneListRow>);

impl fmt::Display for LanePage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.rows.is_empty() && self.0.omission.is_none() {
            return formatter.write_str("no lanes are seated; next: spice lane add [WORKTREE]");
        }
        let mut lines = self
            .0
            .rows
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if let Some(omission) = &self.0.omission {
            lines.push(omission.to_string());
        }
        formatter.write_str(&lines.join("\n"))
    }
}

#[derive(Clone, Default)]
struct LaneListOptions {
    limit: Option<usize>,
    max_bytes: Option<usize>,
    after: Option<CanonicalUuid>,
}

impl LaneListOptions {
    fn from_matches(matches: &ArgMatches) -> Result<Self, Outcome<LanePage>> {
        let limit = numeric_option(matches, "limit", "lane::list-limit")?;
        let max_bytes = numeric_option(matches, "max-bytes", "lane::list-max-bytes")?;
        let _ = lane_list_budget(limit, max_bytes)?;
        let after = matches
            .get_one::<String>("after")
            .map(|written| {
                written.parse::<CanonicalUuid>().map_err(|error| {
                    refusal(
                        "lane::list-after",
                        Some(written.clone()),
                        &error.to_string(),
                    )
                })
            })
            .transpose()?;
        Ok(Self {
            limit,
            max_bytes,
            after,
        })
    }

    fn budget(&self) -> wire::output::Budget {
        lane_list_budget(self.limit, self.max_bytes)
            .expect("CLI parsing and test fixtures admit lane list limits")
    }
}

#[derive(Clone, Copy)]
struct LaneListCandidate<'a> {
    lane: CanonicalUuid,
    name: Option<&'a str>,
    seated: bool,
    claim: Option<&'a str>,
    address: &'a SeatAddress,
}

/// One lane, its display name, its claim, and the peer address its seat
/// offers now.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct LaneListRow {
    lane: CanonicalUuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    seated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim: Option<String>,
    address: SeatAddress,
}

impl fmt::Display for LaneListRow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "lane {}", self.lane)?;
        if let Some(name) = &self.name {
            write!(formatter, " name={name:?}")?;
        }
        write!(
            formatter,
            " seated={} claim={} address={}",
            self.seated,
            self.claim.as_deref().unwrap_or("-"),
            self.address
        )
    }
}

fn lane_list(
    board: &BoardAnswer,
    options: &LaneListOptions,
) -> Result<LanePage, Outcome<LanePage>> {
    let actor = match board.principal {
        Actor::Lane(lane) => Some(lane),
        Actor::Operator | Actor::Spice | Actor::Human(_) => None,
    };
    lane_list_page(lane_list_candidates(board)?, actor, options).map(LanePage)
}

fn lane_list_candidates(
    board: &BoardAnswer,
) -> Result<Vec<LaneListCandidate<'_>>, Outcome<LanePage>> {
    let mut claims = BTreeMap::new();
    for task in &board.tasks {
        let Some(claim) = task
            .claim
            .as_ref()
            .filter(|claim| claim.body.disposition == ClaimDisposition::Active)
        else {
            continue;
        };
        if claims
            .insert(claim.body.lane, task.statement.body.handle.as_str())
            .is_some()
        {
            return Err(protocol_fault(
                "lane::active-claim-ambiguous",
                "one lane has more than one active claim in the board snapshot",
            ));
        }
    }
    board
        .residency
        .body
        .lanes
        .iter()
        .map(|row| {
            let address = board.addresses.get(&row.lane).ok_or_else(|| {
                protocol_fault(
                    "lane::list-address-missing",
                    "the board snapshot names no seat address for a residency lane",
                )
            })?;
            Ok(LaneListCandidate {
                lane: row.lane,
                name: row.name.as_deref(),
                seated: row.disposition == "occupied" && row.occupant.is_some(),
                claim: claims.get(&row.lane).copied(),
                address,
            })
        })
        .collect()
}

fn lane_list_page(
    mut candidates: Vec<LaneListCandidate<'_>>,
    actor: Option<CanonicalUuid>,
    options: &LaneListOptions,
) -> Result<wire::output::Bounded<LaneListRow>, Outcome<LanePage>> {
    candidates.sort_by(|left, right| {
        lane_relevance(*left, actor)
            .cmp(&lane_relevance(*right, actor))
            .then_with(|| left.lane.cmp(&right.lane))
    });
    if let Some(after) = options.after {
        let Some(index) = candidates.iter().position(|row| row.lane == after) else {
            return Err(refusal(
                "lane::list-after-unknown",
                Some(after.to_string()),
                "name a lane present in the current residency table",
            ));
        };
        candidates = candidates.split_off(index);
    }

    let mut admission = wire::output::Admission::new(options.budget());
    for (index, candidate) in candidates.iter().enumerate() {
        if admission.is_closed() {
            admission.withhold(candidates.len() - index);
            break;
        }
        admission
            .offer(LaneListRow {
                lane: candidate.lane,
                name: candidate.name.map(str::to_owned),
                seated: candidate.seated,
                claim: candidate.claim.map(str::to_owned),
                address: candidate.address.clone(),
            })
            .expect("a lane list row always serializes");
    }
    Ok(admission.finish(|resume| lane_list_continuation(options, resume)))
}

fn lane_relevance(candidate: LaneListCandidate<'_>, actor: Option<CanonicalUuid>) -> u8 {
    if Some(candidate.lane) == actor {
        0
    } else if candidate.seated || candidate.claim.is_some() {
        1
    } else {
        2
    }
}

fn lane_list_continuation(
    options: &LaneListOptions,
    resume: wire::output::Resume<'_, LaneListRow>,
) -> wire::SpiceCommand {
    let (row, widened) = match resume {
        wire::output::Resume::At(row) => (row, None),
        wire::output::Resume::Widened { record, bytes } => (record, Some(bytes)),
        wire::output::Resume::Export(row) => (row, Some(wire::output::INTERACTIVE.maximum().bytes)),
    };
    let limits = options.budget().resume();
    wire::SpiceCommand::new(
        "lane",
        [
            "list".to_owned(),
            "--limit".to_owned(),
            limits.rows.to_string(),
            "--max-bytes".to_owned(),
            widened.unwrap_or(limits.bytes).to_string(),
            "--after".to_owned(),
            row.lane.to_string(),
        ],
    )
    .expect("lane list continuation has structural arguments")
}

fn numeric_option(
    matches: &ArgMatches,
    name: &str,
    reason: &str,
) -> Result<Option<usize>, Outcome<LanePage>> {
    matches
        .get_one::<String>(name)
        .map(|written| {
            written.parse::<usize>().map_err(|_| {
                refusal(
                    reason,
                    Some(written.clone()),
                    &format!("name a non-negative integer after --{name}"),
                )
            })
        })
        .transpose()
}

fn lane_list_budget(
    rows: Option<usize>,
    bytes: Option<usize>,
) -> Result<wire::output::Budget, Outcome<LanePage>> {
    wire::output::INTERACTIVE
        .budget(wire::output::Request {
            rows,
            bytes,
            text: None,
        })
        .map_err(|error| {
            refusal(
                "lane::list-limits",
                Some(error.to_string()),
                &error.to_string(),
            )
        })
}

fn refusal<B>(reason: &str, observed: Option<String>, detail: &str) -> Outcome<B> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(reason.to_owned(), None, None, observed).with_detail(detail),
    ))
}

fn protocol_fault<B>(reason: &str, detail: &str) -> Outcome<B> {
    Outcome::Fault(Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(value: u128) -> CanonicalUuid {
        CanonicalUuid::new(uuid::Uuid::from_u128(value))
    }

    static VACANT: SeatAddress = SeatAddress::Vacant;

    fn candidates() -> Vec<LaneListCandidate<'static>> {
        (1..=206)
            .map(|value| LaneListCandidate {
                lane: uuid(value),
                name: None,
                seated: value % 2 == 0,
                claim: (value % 5 == 0).then_some("TASK-claim"),
                address: &VACANT,
            })
            .collect()
    }

    #[test]
    fn hundreds_of_lanes_page_actor_then_active_rows_without_gaps() {
        let actor = uuid(206);
        let first = lane_list_page(candidates(), Some(actor), &LaneListOptions::default())
            .expect("default lane page renders");
        assert_eq!(first.rows.len(), 20);
        assert_eq!(first.rows[0].lane, actor, "actor lane is never displaced");
        assert_eq!(first.rows[1].lane, uuid(2));
        assert_eq!(first.rows[2].lane, uuid(4));
        assert_eq!(first.rows[3].lane, uuid(5));
        let omission = first.omission.expect("hundreds of lanes need another page");
        assert_eq!(
            (omission.population, omission.shown, omission.withheld),
            (206, 20, 186)
        );
        assert_eq!(
            omission.continuation.arguments()[..5],
            ["lane", "list", "--limit", "20", "--max-bytes"]
        );

        let mut options = LaneListOptions {
            limit: Some(17),
            max_bytes: Some(wire::output::INTERACTIVE.maximum().bytes),
            after: None,
        };
        let mut seen = Vec::new();
        loop {
            let page = lane_list_page(candidates(), Some(actor), &options)
                .expect("continuation cursor stays in the residency table");
            seen.extend(page.rows.into_iter().map(|row| row.lane));
            let Some(omission) = page.omission else {
                break;
            };
            let arguments = omission.continuation.arguments();
            options.after = Some(
                arguments
                    .last()
                    .expect("continuation carries a cursor")
                    .parse()
                    .expect("continuation cursor is a lane UUID"),
            );
        }

        let mut expected = vec![actor];
        expected.extend(
            (1..206)
                .filter(|value| value % 2 == 0 || value % 5 == 0)
                .map(uuid),
        );
        expected.extend(
            (1..206)
                .filter(|value| value % 2 != 0 && value % 5 != 0)
                .map(uuid),
        );
        assert_eq!(seen, expected);
    }

    #[test]
    fn lane_list_parser_owns_limits_and_cursor() {
        let after = uuid(17);
        let matches = crate::cli::command()
            .try_get_matches_from([
                "spice",
                "lane",
                "list",
                "--limit",
                "23",
                "--max-bytes",
                "4096",
                "--after",
                &after.to_string(),
            ])
            .expect("bounded lane list parses");
        let (_, lane) = matches.subcommand().expect("lane command");
        let (_, list) = lane.subcommand().expect("lane list command");
        let options = LaneListOptions::from_matches(list).expect("lane list limits are admitted");
        assert_eq!(options.limit, Some(23));
        assert_eq!(options.max_bytes, Some(4096));
        assert_eq!(options.after, Some(after));

        let too_many = crate::cli::command()
            .try_get_matches_from(["spice", "lane", "list", "--limit", "501"])
            .expect("syntax accepts a numeric limit for semantic admission");
        let (_, lane) = too_many.subcommand().expect("lane command");
        let (_, list) = lane.subcommand().expect("lane list command");
        assert!(LaneListOptions::from_matches(list).is_err());
    }
}
