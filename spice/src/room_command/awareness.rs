//! `spice room awareness HANDLE`: session-profile requests for one Design.
//!
//! Each row is one admitted `room::noticed` record, oldest first: the requested
//! lane and continuing vendor session, the exact opportunity, supporting task
//! or conversation evidence, Design statement, and matched terms as every
//! surface shows them. Rows page whole in journal order; a page that withholds
//! any continues at the first withheld one. Reading changes nothing.

use std::fmt;

use clap::ArgMatches;
use serde::Serialize;
use spice::repository::Repository;
use wire::output::Bounded;
use wire::rooms::{DesignItemStanding, DesignMaturity, DesignMaturityState, DesignNoticed};
use wire::{CanonicalUuid, FaultClass, Outcome, Reply, ReplyPosition, Seq, TaskHandle, TaskStamp};

use super::{Failure, current_handle, discover, fault, store};
use crate::read_page::Page;

/// What a continuation's `--after` names.
const CURSOR: &str = "an awareness record of this Design";

/// One page of completion requests for one Design.
#[derive(Debug, Serialize)]
pub(crate) struct DesignAwareness {
    design: TaskHandle,
    anchor: TaskStamp,
    page: Bounded<Noticed>,
    maturity: DesignMaturity,
    /// Maturity judged against the Design's current statement.
    state: DesignMaturityState,
    /// Each current question item's standing.
    items: Vec<ItemStanding>,
    effect: &'static str,
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
    let (named, anchor) = super::named_room(matches)?;
    let requested = page
        .after()
        .map(|after| {
            after
                .parse::<Seq>()
                .map_err(|_| page.cursor_refusal(CURSOR))
        })
        .transpose()?;
    let after = requested.and_then(|seq| seq.get().checked_sub(1).and_then(Seq::new));
    let retain = page.retain();
    let (cursor, design, maturity, acceptance, population, rows) = journal
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
            let population = read.design_notice_count(&anchor, after).map_err(store)?;
            let budget = journal::FactReadBudget::new(
                retain.clamp(1, journal::FactReadBudget::RETAINED.rows()),
                journal::FactReadBudget::RETAINED.decoded_bytes(),
            )
            .map_err(store)?;
            let mut rows = Vec::with_capacity(retain.min(population));
            let mut from = after;
            while rows.len() < retain.min(population) {
                let fetched = read.design_notices(&anchor, from, budget).map_err(store)?;
                let wanted = retain - rows.len();
                for envelope in fetched.rows.into_iter().take(wanted) {
                    rows.push(noticed(read, envelope)?);
                }
                match fetched.continuation {
                    Some(next) => from = Some(next),
                    None => break,
                }
            }
            Ok((cursor, design, maturity, acceptance, population, rows))
        })
        .map_err(|error| match error {
            journal::ReadError::Journal(error) => store(error),
            journal::ReadError::Answer(failure) => failure,
        })?;
    if let Some(notice) = crate::board_command::rename_notice(&named, &design) {
        eprintln!("{notice}");
    }
    let page = page.shown(
        rows.into_iter()
            .map(|row| {
                let key = row.seq.to_string();
                (row, key)
            })
            .collect(),
        population,
    )?;
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
            page,
            maturity,
            state,
            items,
            effect: "advisory only; neither blocks nor authorizes a Design decision",
        },
    ))
}

/// The row one indexed notice envelope reads as.
fn noticed(
    _read: &journal::JournalRead<'_>,
    envelope: wire::Envelope<wire::AnyFact>,
) -> Result<Noticed, Failure> {
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

impl fmt::Display for DesignAwareness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "design {} awareness (background notices; no attention or acknowledgment)",
            self.design
        )?;
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
        for row in &self.page.rows {
            write!(formatter, "\n{row}")?;
        }
        match &self.page.omission {
            Some(omission) => write!(formatter, "\n{omission}"),
            None => Ok(()),
        }
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
