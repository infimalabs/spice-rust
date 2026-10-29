//! `spice room awareness HANDLE [--shadows]`: session-profile requests for
//! one Design, or the shadows its floor refused.
//!
//! Each row is one admitted `room::noticed` record, oldest first: the requested
//! lane and continuing vendor session, the exact opportunity, supporting task
//! or conversation evidence, Design statement, and matched terms as every
//! surface shows them. With `--shadows` each row is instead one
//! `room::shadowed` record: a session whose best Design was this one but fell
//! short of its reach floor, with its score, the floor, and what that lane
//! later did in the room. Rows page whole in journal order; a page that
//! withholds any continues at the first withheld one. Reading changes nothing.

use std::fmt;

use clap::ArgMatches;
use serde::Serialize;
use spice::repository::Repository;
use wire::output::Bounded;
use wire::rooms::{
    DesignItemStanding, DesignMaturity, DesignMaturityState, DesignNoticed, DesignShadowed,
    RoomContributed, RoomLeft,
};
use wire::{CanonicalUuid, FaultClass, Outcome, Reply, ReplyPosition, Seq, TaskHandle, TaskStamp};

use super::{Failure, current_handle, discover, fault, store};
use crate::read_page::Page;

/// What a continuation's `--after` names.
const CURSOR: &str = "an awareness record of this Design";
/// What a shadow continuation's `--after` names.
const SHADOW_CURSOR: &str = "a shadow record of this Design";

/// One page of completion requests, or of shadows, for one Design.
#[derive(Debug, Serialize)]
pub(crate) struct DesignAwareness {
    design: TaskHandle,
    anchor: TaskStamp,
    page: Rows,
    maturity: DesignMaturity,
    /// Maturity judged against the Design's current statement.
    state: DesignMaturityState,
    /// Each current question item's standing.
    items: Vec<ItemStanding>,
    effect: &'static str,
}

/// The rows one awareness page carries.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Rows {
    Noticed(Bounded<Noticed>),
    Shadowed(Bounded<Shadowed>),
}

/// One current question item with its text.
#[derive(Debug, Serialize)]
pub(crate) struct ItemStanding {
    #[serde(flatten)]
    standing: DesignItemStanding,
    text: String,
}

/// One admitted awareness record.
#[derive(Debug, Serialize)]
pub(crate) struct Noticed {
    seq: Seq,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    session: wire::rooms::DesignSession,
    opportunity: wire::rooms::DesignNoticeOpportunity,
    evidence: wire::rooms::DesignNoticeEvidence,
    statement: Seq,
    reach: Option<wire::rooms::DesignReach>,
    matching: Option<wire::rooms::DesignNoticeMatch>,
    terms: Vec<String>,
    /// Question items asked, in asked order; empty asks about the whole Design.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    asks: Vec<wire::rooms::DesignAsk>,
}

/// One admitted shadow record and what its lane did in the room afterwards.
#[derive(Debug, Serialize)]
pub(crate) struct Shadowed {
    seq: Seq,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    session: wire::rooms::DesignSession,
    opportunity: wire::rooms::DesignNoticeOpportunity,
    evidence: wire::rooms::DesignNoticeEvidence,
    statement: Seq,
    terms: Vec<String>,
    reach: wire::rooms::DesignReach,
    floor: wire::rooms::DesignSimilarity,
    score: wire::rooms::DesignSimilarity,
    generation: wire::rooms::DesignFitGeneration,
    outcome: ShadowOutcome,
}

/// The first thing a shadowed lane did in the Design's room after its shadow,
/// or the Design claim it won since.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum ShadowOutcome {
    /// The lane contributed to the Design.
    Contributed { at: Seq },
    /// The lane left the Design's room.
    Left { at: Seq },
    /// The lane won the Design's claim.
    Captured { at: Seq },
    /// Nothing yet.
    None,
}

impl ShadowOutcome {
    fn label(self) -> String {
        match self {
            Self::Contributed { at } => format!("contributed at {at}"),
            Self::Left { at } => format!("left at {at}"),
            Self::Captured { at } => format!("captured at {at}"),
            Self::None => "none".to_owned(),
        }
    }
}

/// Read one bounded page of a Design's awareness records from the working
/// repository.
pub(crate) fn run(
    matches: &ArgMatches,
    contract: &crate::cli::Contract,
) -> Outcome<DesignAwareness> {
    match discover() {
        Ok(repository) => run_at(&repository, matches, contract),
        Err(failure) => failure.outcome(),
    }
}

/// [`run`] against `repository`.
pub(crate) fn run_at(
    repository: &Repository,
    matches: &ArgMatches,
    contract: &crate::cli::Contract,
) -> Outcome<DesignAwareness> {
    match super::open(repository) {
        Ok((_, journal)) => answer(&journal, matches, contract.path),
        Err(failure) => failure.outcome(),
    }
}

/// One page of the awareness records of the Design `matches` names.
pub(crate) fn answer(
    journal: &journal::JournalReader,
    matches: &ArgMatches,
    path: &[&str],
) -> Outcome<DesignAwareness> {
    match awareness(journal, matches, path) {
        Ok((position, body)) => Outcome::Reply(Reply { position, body }),
        Err(failure) => failure.outcome(),
    }
}

fn awareness(
    journal: &journal::JournalReader,
    matches: &ArgMatches,
    path: &[&str],
) -> Result<(ReplyPosition, DesignAwareness), Failure> {
    let page = Page::from_matches(matches, path)?;
    let shadows = matches.get_flag("shadows");
    let (named, anchor) = super::named_room(matches)?;
    let requested = page
        .after()
        .map(|after| {
            after
                .parse::<Seq>()
                .map_err(|_| page.cursor_refusal(if shadows { SHADOW_CURSOR } else { CURSOR }))
        })
        .transpose()?;
    let after = requested.and_then(|seq| seq.get().checked_sub(1).and_then(Seq::new));
    let retain = page.retain();
    let (cursor, design, maturity, acceptance, rows) = journal
        .read_with(|read| -> Result<_, Failure> {
            let design = current_handle(read, &anchor)?;
            let cursor = read.cursor().map_err(store)?.ok_or_else(|| {
                fault(
                    FaultClass::Store,
                    "room::cursor-missing",
                    "the journal names a task but no read position",
                )
            })?;
            let maturity = read.design_maturity(&anchor).map_err(store)?;
            let acceptance = read
                .task_head(&anchor)
                .map_err(store)?
                .map(|head| head.body().acceptance.clone())
                .unwrap_or_default();
            let population = if shadows {
                read.design_shadow_count(&anchor, after)
            } else {
                read.design_notice_count(&anchor, after)
            }
            .map_err(store)?;
            let budget = journal::FactReadBudget::new(
                retain.clamp(1, journal::FactReadBudget::RETAINED.rows()),
                journal::FactReadBudget::RETAINED.decoded_bytes(),
            )
            .map_err(store)?;
            let mut rows = Vec::with_capacity(retain.min(population));
            let mut from = after;
            while rows.len() < retain.min(population) {
                let fetched = if shadows {
                    read.design_shadows(&anchor, from, budget)
                } else {
                    read.design_notices(&anchor, from, budget)
                }
                .map_err(store)?;
                let wanted = retain - rows.len();
                rows.extend(fetched.rows.into_iter().take(wanted));
                match fetched.continuation {
                    Some(next) => from = Some(next),
                    None => break,
                }
            }
            let rows = if shadows {
                Rows::Shadowed(page.shown(shadowed_rows(read, &anchor, rows)?, population)?)
            } else {
                let rows = rows
                    .into_iter()
                    .map(noticed)
                    .collect::<Result<Vec<_>, _>>()?;
                Rows::Noticed(page.shown(keyed(rows, |row| row.seq), population)?)
            };
            Ok((cursor, design, maturity, acceptance, rows))
        })
        .map_err(|error| match error {
            journal::ReadError::Journal(error) => store(error),
            journal::ReadError::Answer(failure) => failure,
        })?;
    if let Some(notice) = crate::board_command::rename_notice(&named, &design) {
        eprintln!("{notice}");
    }

    let state = maturity.state(&acceptance);
    let items = maturity
        .items
        .standings(&acceptance)
        .into_iter()
        .zip(acceptance)
        .map(|(standing, text)| ItemStanding { standing, text })
        .collect();
    Ok((
        ReplyPosition::Snapshot(cursor),
        DesignAwareness {
            design,
            anchor,
            page: rows,
            maturity,
            state,
            items,
            effect: "advisory only; neither blocks nor authorizes a Design decision",
        },
    ))
}

/// Each row beside its continuation key, the journal position.
fn keyed<T>(rows: Vec<T>, seq: impl Fn(&T) -> Seq) -> Vec<(T, String)> {
    rows.into_iter()
        .map(|row| {
            let key = seq(&row).to_string();
            (row, key)
        })
        .collect()
}

/// The row one indexed notice envelope reads as.
fn noticed(envelope: wire::Envelope<wire::AnyFact>) -> Result<Noticed, Failure> {
    let Some(body) = envelope.fact.read::<DesignNoticed>() else {
        return Err(fault(
            FaultClass::Store,
            "room::awareness-index-invalid",
            "the Design notice index returned another fact kind",
        ));
    };
    Ok(Noticed {
        seq: envelope.seq,
        lane: body.lane,
        occupant: body.occupant,
        session: body.session,
        opportunity: body.opportunity,
        evidence: body.evidence,
        statement: body.statement,
        reach: body.reach,
        matching: body.matching,
        terms: wire::rooms::shown_notice_terms(&body.terms)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        asks: body.asks,
    })
}

/// Each indexed shadow envelope as a row with its lane's later outcome,
/// keyed by journal position.
fn shadowed_rows(
    read: &journal::JournalRead<'_>,
    anchor: &TaskStamp,
    envelopes: Vec<wire::Envelope<wire::AnyFact>>,
) -> Result<Vec<(Shadowed, String)>, Failure> {
    let mut shadows = Vec::with_capacity(envelopes.len());
    for envelope in envelopes {
        let Some(body) = envelope.fact.read::<DesignShadowed>() else {
            return Err(fault(
                FaultClass::Store,
                "room::awareness-index-invalid",
                "the Design shadow index returned another fact kind",
            ));
        };
        shadows.push((envelope.seq, body));
    }
    let outcomes = RoomOutcomes::after(read, anchor, shadows.iter().map(|(seq, _)| *seq).min())?;
    let rows = shadows
        .into_iter()
        .map(|(seq, body)| {
            let outcome = outcomes.of(body.lane, seq);
            Shadowed {
                seq,
                lane: body.lane,
                occupant: body.occupant,
                session: body.session,
                opportunity: body.opportunity,
                evidence: body.evidence,
                statement: body.statement,
                terms: wire::rooms::shown_notice_terms(&body.terms)
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                reach: body.reach,
                floor: body.floor,
                score: body.score,
                generation: body.generation,
                outcome,
            }
        })
        .collect();
    Ok(keyed(rows, |row| row.seq))
}

/// Every lane's room contributions and leaves in one Design's room from a
/// position on, and the Design's current claim, read once for a page.
struct RoomOutcomes {
    /// Each lane's contribution and leave positions, ascending.
    contributions: std::collections::BTreeMap<CanonicalUuid, Vec<Seq>>,
    leaves: std::collections::BTreeMap<CanonicalUuid, Vec<Seq>>,
    /// The lane holding or last holding the Design's claim and when it won.
    claim: Option<(CanonicalUuid, Seq)>,
}

impl RoomOutcomes {
    fn after(
        read: &journal::JournalRead<'_>,
        anchor: &TaskStamp,
        earliest: Option<Seq>,
    ) -> Result<Self, Failure> {
        let mut outcomes = Self {
            contributions: std::collections::BTreeMap::new(),
            leaves: std::collections::BTreeMap::new(),
            claim: read
                .claim_head(anchor)
                .map_err(store)?
                .map(|head| (head.body().lane, head.body().claimed)),
        };
        let Some(earliest) = earliest else {
            return Ok(outcomes);
        };
        let mut after = Some(earliest);
        loop {
            let page = read
                .room_publications(anchor, after, journal::FactReadBudget::RETAINED)
                .map_err(store)?;
            for envelope in &page.rows {
                if let Some(contributed) = envelope.fact.read::<RoomContributed>() {
                    outcomes
                        .contributions
                        .entry(contributed.lane)
                        .or_default()
                        .push(envelope.seq);
                } else if let Some(left) = envelope.fact.read::<RoomLeft>() {
                    outcomes
                        .leaves
                        .entry(left.lane)
                        .or_default()
                        .push(envelope.seq);
                }
            }
            let Some(continuation) = page.continuation else {
                return Ok(outcomes);
            };
            after = Some(continuation);
        }
    }

    /// The earliest thing `lane` did after `shadow`, a claim win counting at
    /// its position.
    fn of(&self, lane: CanonicalUuid, shadow: Seq) -> ShadowOutcome {
        let first = |positions: Option<&Vec<Seq>>| {
            positions.and_then(|positions| positions.iter().copied().find(|at| *at > shadow))
        };
        [
            first(self.contributions.get(&lane)).map(|at| (at, ShadowOutcome::Contributed { at })),
            first(self.leaves.get(&lane)).map(|at| (at, ShadowOutcome::Left { at })),
            self.claim
                .filter(|(holder, claimed)| *holder == lane && *claimed > shadow)
                .map(|(_, at)| (at, ShadowOutcome::Captured { at })),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|(at, _)| *at)
        .map_or(ShadowOutcome::None, |(_, outcome)| outcome)
    }
}

impl fmt::Display for DesignAwareness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.page {
            Rows::Noticed(_) => write!(
                formatter,
                "design {} awareness (background notices; no attention or acknowledgment)",
                self.design
            )?,
            Rows::Shadowed(_) => write!(
                formatter,
                "design {} shadows (sessions whose best Design fell short of its floor; \
                 nothing was delivered)",
                self.design
            )?,
        }
        write!(
            formatter,
            "\n{}",
            maturity_summary(&self.maturity, &self.state)
        )?;
        for item in &self.items {
            let (charges, lanes) = (item.standing.charges, item.standing.lanes);
            write!(
                formatter,
                "\nitem {} {}: {charges} charge{} from {lanes} lane{}: {}",
                item.standing.question,
                item.standing.state.label(),
                if charges == 1 { "" } else { "s" },
                if lanes == 1 { "" } else { "s" },
                item.text,
            )?;
        }
        for head in self.maturity.recent_heads.iter().rev() {
            write!(
                formatter,
                "\nstatement {}: {} asked; {} contributed, {} declined, {} unanswered{}",
                head.statement,
                head.requested_lanes.len(),
                head.requested_contributors(),
                head.requested_decliners(),
                head.requested_unanswered(),
                if head.yield_truncated() {
                    " (bounded)"
                } else {
                    ""
                },
            )?;
        }
        if self
            .maturity
            .recent_heads
            .iter()
            .any(|head| head.requested_decliners() > 0)
        {
            write!(
                formatter,
                "\ndecline clauses: spice room history {}",
                self.design
            )?;
        }
        write!(formatter, "\n{}", self.effect)?;
        match &self.page {
            Rows::Noticed(page) => rows(formatter, page),
            Rows::Shadowed(page) => rows(formatter, page),
        }
    }
}

fn rows<T: fmt::Display>(formatter: &mut fmt::Formatter<'_>, page: &Bounded<T>) -> fmt::Result {
    for row in &page.rows {
        write!(formatter, "\n{row}")?;
    }
    match &page.omission {
        Some(omission) => write!(formatter, "\n{omission}"),
        None => Ok(()),
    }
}

fn maturity_summary(maturity: &DesignMaturity, state: &DesignMaturityState) -> String {
    let contributions = maturity.contributions;
    let contributing_lanes = maturity.contributing_lanes.len();
    let requested_lanes = maturity.requested_lanes.len();
    format!(
        "maturity {}; {contributions} contribution{} from {contributing_lanes} lane{}; \
         requests={} to {requested_lanes} lane{}",
        state.summary(),
        if contributions == 1 { "" } else { "s" },
        if contributing_lanes == 1 { "" } else { "s" },
        maturity.requests,
        if requested_lanes == 1 { "" } else { "s" },
    )
}

impl fmt::Display for Noticed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let opportunity = spice::rooms::awareness::notice_opportunity_text(&self.opportunity);
        let evidence = spice::rooms::awareness::notice_evidence_text(&self.evidence);
        write!(
            formatter,
            "{} lane {} occupant {} session {}/{} opportunity {opportunity} evidence {evidence} \
             statement {}",
            self.seq,
            self.lane,
            self.occupant,
            self.session.vendor,
            self.session.identity,
            self.statement,
        )?;
        match (&self.reach, &self.matching) {
            (Some(reach), Some(matching)) => write!(
                formatter,
                " reach {} score {} floor {} fit {}",
                match reach {
                    wire::rooms::DesignReach::Wide => "wide",
                    wire::rooms::DesignReach::Narrow => "narrow",
                },
                matching.score(),
                matching.floor(),
                matching.generation().as_str(),
            )?,
            (Some(reach), None) => write!(
                formatter,
                " reach {} score unavailable",
                match reach {
                    wire::rooms::DesignReach::Wide => "wide",
                    wire::rooms::DesignReach::Narrow => "narrow",
                },
            )?,
            (None, _) => write!(formatter, " reach unavailable score unavailable")?,
        }
        if !self.terms.is_empty() {
            write!(formatter, " terms {}", self.terms.join(", "))?;
        }
        for (index, ask) in self.asks.iter().enumerate() {
            let lead = if index == 0 { " asks" } else { "," };
            write!(
                formatter,
                "{lead} question {} ({})",
                ask.question,
                ask.state.label()
            )?;
        }
        Ok(())
    }
}

impl fmt::Display for Shadowed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let opportunity = spice::rooms::awareness::notice_opportunity_text(&self.opportunity);
        let evidence = spice::rooms::awareness::notice_evidence_text(&self.evidence);
        write!(
            formatter,
            "{} lane {} occupant {} session {}/{} opportunity {opportunity} evidence {evidence} \
             statement {} reach {} score {} floor {} generation {} outcome {}",
            self.seq,
            self.lane,
            self.occupant,
            self.session.vendor,
            self.session.identity,
            self.statement,
            self.reach,
            self.score.get(),
            self.floor.get(),
            self.generation.as_str(),
            self.outcome.label(),
        )?;
        if !self.terms.is_empty() {
            write!(formatter, " terms {}", self.terms.join(", "))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(byte: u8) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-0000000000{byte:02x}")
            .parse()
            .expect("fixture uuid")
    }

    fn conversation() -> wire::rooms::DesignConversationEvidence {
        wire::rooms::DesignConversationEvidence {
            locus: wire::body::TranscriptLocus {
                source: Some(uuid(9)),
                member: 3,
                offset: 5,
                length: 7,
                event: 0,
                line: 1,
                column: 1,
            },
            digest: wire::Digest::compute(wire::Domain::RoomSource, b"fixture conversation"),
        }
    }

    fn row(
        opportunity: wire::rooms::DesignNoticeOpportunity,
        evidence: wire::rooms::DesignNoticeEvidence,
    ) -> Noticed {
        Noticed {
            seq: Seq::new(1).expect("fixture sequence"),
            lane: uuid(1),
            occupant: uuid(2),
            session: wire::rooms::DesignSession {
                vendor: "omp".parse().expect("fixture vendor"),
                identity: "fixture-session".to_owned(),
            },
            opportunity,
            evidence,
            statement: Seq::new(2).expect("fixture statement"),
            terms: vec!["rendering".to_owned()],
            reach: Some(wire::rooms::DesignReach::Wide),
            matching: None,
            asks: Vec::new(),
        }
    }

    #[test]
    fn notice_rows_render_semantic_sources_without_debug_structs() {
        use wire::rooms::{
            DesignNoticeEvidence as Evidence, DesignNoticeOpportunity as Opportunity, DesignSupport,
        };

        let task = wire::TaskStamp::for_micros(1_800_000_000_000_000).expect("fixture task stamp");
        let statement = Seq::new(3).expect("fixture statement");
        let completed = Seq::new(4).expect("fixture completion");
        let terminal = Seq::new(5).expect("fixture terminal");
        let claim = wire::SequenceCode::new(6);
        let conversation = conversation();
        let rows = [
            row(
                Opportunity::TaskCompletion {
                    task: task.clone(),
                    completed,
                },
                Evidence::Task {
                    task: task.clone(),
                    statement,
                    completed,
                    support: DesignSupport::Implemented,
                },
            ),
            row(
                Opportunity::TaskClaim {
                    task: task.clone(),
                    claim,
                    terminal: None,
                },
                Evidence::TaskClaim {
                    task: task.clone(),
                    statement,
                    claim,
                    terminal: None,
                    support: DesignSupport::Implemented,
                },
            ),
            row(
                Opportunity::TaskClaim {
                    task: task.clone(),
                    claim,
                    terminal: Some(terminal),
                },
                Evidence::TaskClaim {
                    task: task.clone(),
                    statement,
                    claim,
                    terminal: Some(terminal),
                    support: DesignSupport::Implemented,
                },
            ),
            row(
                Opportunity::Conversation {
                    source: conversation.clone(),
                },
                Evidence::Conversation {
                    source: conversation.clone(),
                },
            ),
            row(
                Opportunity::DesignStatement,
                Evidence::Task {
                    task: task.clone(),
                    statement,
                    completed,
                    support: DesignSupport::Implemented,
                },
            ),
            row(
                Opportunity::ProfileReady {
                    horizon: Some(completed),
                },
                Evidence::Task {
                    task: task.clone(),
                    statement,
                    completed,
                    support: DesignSupport::Implemented,
                },
            ),
            row(
                Opportunity::SessionResumed {
                    declaration: completed,
                },
                Evidence::Task {
                    task,
                    statement,
                    completed,
                    support: DesignSupport::Implemented,
                },
            ),
        ]
        .map(|row| row.to_string());

        for rendered in &rows {
            assert_eq!(rendered.lines().count(), 1, "{rendered}");
            for debug in ["{", "CanonicalUuid(", "Digest(", "Some("] {
                assert!(!rendered.contains(debug), "{rendered}");
            }
        }
        let source = conversation
            .locus
            .source
            .expect("fixture conversation has a source")
            .to_string();
        assert_eq!(
            rows[3].matches(&source).count(),
            1,
            "one conversation source is rendered once"
        );
    }

    #[test]
    fn notice_rows_distinguish_recorded_match_from_legacy_absence() {
        let opportunity = wire::rooms::DesignNoticeOpportunity::DesignStatement;
        let evidence = wire::rooms::DesignNoticeEvidence::Conversation {
            source: conversation(),
        };
        let legacy = row(opportunity.clone(), evidence.clone()).to_string();
        assert!(legacy.contains("reach wide score unavailable"), "{legacy}");
        assert!(!legacy.contains("score 0"), "{legacy}");

        let mut recorded = row(opportunity, evidence);
        recorded.matching = wire::rooms::DesignNoticeMatch::new(0.25, 0.1, [7; 32]);
        let recorded = recorded.to_string();
        assert!(
            recorded.contains("reach wide score 0.25 floor 0.1"),
            "{recorded}"
        );
        assert!(!recorded.contains("score unavailable"), "{recorded}");
    }
}
