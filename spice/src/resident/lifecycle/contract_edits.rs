//! Durable claimant steering derived from task contract revisions.

use std::time::Duration;

use journal::{FactCause, JournalGuard};
use wire::{ClaimDisposition, Seq, TaskStamp};

use super::*;

const CONTRACT_FIELDS: [&str; 6] = [
    "title",
    "description",
    "acceptance",
    "priority",
    "project",
    "dependencies",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::resident) struct NoticeAuthority {
    pub(in crate::resident) task: TaskStamp,
    pub(in crate::resident) edit: Seq,
    pub(in crate::resident) claim: Seq,
}

pub(super) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    snapshot: &bus::TaskSnapshot,
    cursor: Option<Seq>,
    lifetime: Duration,
) -> CommandResult<bool> {
    let mut refreshed = None;
    let mut changed = false;
    loop {
        let observed = refreshed.as_ref().unwrap_or(snapshot);
        let mut appended = false;
        for envelope in observed.facts().after(cursor) {
            let Some(edited) = envelope.fact.read::<wire::body::TaskEdited>() else {
                continue;
            };
            let Some(notice) = edited.notice.as_ref() else {
                continue;
            };
            admit_notice(&edited, envelope.seq, notice)?;
            if collapsed_notice(notice) {
                continue;
            }
            if !current_claim_matches(observed, &edited.task, notice) {
                continue;
            }
            let control = control(&edited.task, envelope.seq, notice.claim);
            let directive = identity(&control);
            if super::super::steering::control_standing(bus, directive, notice.lane, &control)?
                .is_some()
            {
                continue;
            }
            let text = text(&edited, envelope.seq, notice, &envelope.actor);
            let (mut proposal, published) = super::super::steering::internal_write(epoch)?;
            proposal.guard(claim_guard(edited.task.clone(), notice.lane, notice.claim));
            super::super::steering::append_internal_directive(
                bus,
                &mut proposal,
                epoch,
                super::super::steering::InternalDirective {
                    directive,
                    lane: notice.lane,
                    text,
                    control,
                    lifetime,
                    cause: FactCause::Root,
                    published,
                },
            )?;
            match commit(bus, proposal) {
                Ok(_) => {
                    let residency = current_residency(bus, epoch)?;
                    super::super::steering::publish_inbox(
                        bus,
                        paths,
                        residency.body(),
                        notice.lane,
                    )?;
                }
                Err(Outcome::Refusal(refusal))
                    if refusal.reason() == "task-contract::claim-moved" => {}
                Err(outcome) => return Err(outcome),
            }
            appended = true;
            changed = true;
            break;
        }
        if !appended {
            return Ok(changed);
        }
        refreshed = Some(bus.task_snapshot().map_err(|error| {
            store_fault("task-contract::snapshot-unreadable", &error.to_string())
        })?);
    }
}

fn admit_notice(
    edited: &wire::body::TaskEdited,
    current: Seq,
    notice: &wire::body::TaskEditNotice,
) -> CommandResult<()> {
    if notice.handle.born().as_ref() != Ok(&edited.task) || notice.previous >= current {
        return Err(store_fault(
            "task-contract::edit-invalid",
            "task edit notice identity or revision range is invalid",
        ));
    }
    if notice.changes.is_empty() {
        return Err(store_fault(
            "task-contract::edit-invalid",
            "task edit notice names no changed contract field",
        ));
    }
    let mut previous = None;
    for change in &notice.changes {
        let Some(position) = CONTRACT_FIELDS
            .iter()
            .position(|field| *field == change.field)
        else {
            return Err(store_fault(
                "task-contract::edit-invalid",
                "task edit notice names an unknown contract field",
            ));
        };
        if previous.is_some_and(|previous| position <= previous)
            || [change.before.as_str(), change.after.as_str()]
                .into_iter()
                .any(|summary| {
                    summary.chars().count() > wire::body::TASK_EDIT_FIELD_SUMMARY_CHARS
                        || summary.contains('\r')
                        || summary.contains('\n')
                })
        {
            return Err(store_fault(
                "task-contract::edit-invalid",
                "task edit notice fields are unordered or unbounded",
            ));
        }
        previous = Some(position);
    }
    Ok(())
}

fn collapsed_notice(notice: &wire::body::TaskEditNotice) -> bool {
    notice
        .changes
        .iter()
        .all(|change| change.before == change.after)
}

fn current_claim_matches(
    snapshot: &bus::TaskSnapshot,
    task: &TaskStamp,
    notice: &wire::body::TaskEditNotice,
) -> bool {
    snapshot
        .tasks()
        .binary_search_by(|heads| heads.statement().body().born.cmp(task))
        .ok()
        .and_then(|index| snapshot.tasks()[index].claim())
        .is_some_and(|claim| {
            claim.body().disposition == ClaimDisposition::Active
                && claim.body().lane == notice.lane
                && claim.body().claimed == notice.claim
        })
}

fn claim_guard(task: TaskStamp, lane: CanonicalUuid, claimed: Seq) -> JournalGuard {
    JournalGuard::new(move |read| {
        let current = read.claim_head(&task)?;
        if current.is_some_and(|head| {
            head.body().disposition == ClaimDisposition::Active
                && head.body().lane == lane
                && head.body().claimed == claimed
        }) {
            Ok(())
        } else {
            Err(journal::GuardError::refused("task-contract::claim-moved"))
        }
    })
}

fn control(task: &TaskStamp, edit: Seq, claim: Seq) -> String {
    format!(
        "{}{task}:{edit}:{claim}",
        spice::session_meter::TASK_CONTRACT_CONTROL_PREFIX
    )
}

pub(in crate::resident) fn notice_authority(
    control: Option<&str>,
) -> Result<Option<NoticeAuthority>, String> {
    let Some(control) = control else {
        return Ok(None);
    };
    let Some(encoded) = control.strip_prefix(spice::session_meter::TASK_CONTRACT_CONTROL_PREFIX)
    else {
        return Ok(None);
    };
    let mut fields = encoded.split(':');
    let task = fields
        .next()
        .ok_or_else(|| "task-contract control omitted its task".to_owned())?
        .parse()
        .map_err(|error| format!("task-contract control has an invalid task: {error}"))?;
    let edit = control_seq(fields.next(), "edit")?;
    let claim = control_seq(fields.next(), "claim")?;
    if fields.next().is_some() {
        return Err("task-contract control carries trailing fields".to_owned());
    }
    Ok(Some(NoticeAuthority { task, edit, claim }))
}

fn control_seq(value: Option<&str>, field: &str) -> Result<Seq, String> {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(Seq::new)
        .ok_or_else(|| format!("task-contract control has an invalid {field} position"))
}

fn identity(control: &str) -> CanonicalUuid {
    let digest = blake3::hash(format!("spice:{control}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    CanonicalUuid::new(uuid::Uuid::from_bytes(bytes))
}

fn text(
    edited: &wire::body::TaskEdited,
    current: Seq,
    notice: &wire::body::TaskEditNotice,
    editor: &Actor,
) -> String {
    let changes = notice
        .changes
        .iter()
        .map(|change| {
            format!(
                "{}: {:?} -> {:?}",
                change.field, change.before, change.after
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "Task contract changed for {}. Statement revision {} -> {} (version {}) by {}. Changed fields: {}. Acknowledge this revision before continuing.",
        notice.handle, notice.previous, current, edited.revision, editor, changes,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::resident::lane_surface::{AdvisoryRtkFixture, seed_task_claim};
    use bus::CommitOutcome;
    use journal::{Journal, JournalWrite};
    use wire::{LaneResidency, Origin, Producer, ResidentState, TaskHandle, TaskHead};

    #[test]
    fn directive_identity_is_stable_and_edit_specific() {
        let task: TaskStamp = "1kKRRsDP".parse().expect("task stamp");
        let first = control(
            &task,
            Seq::new(2).expect("edit"),
            Seq::new(1).expect("claim"),
        );
        assert_eq!(identity(&first), identity(&first));
        assert_ne!(
            identity(&first),
            identity(&control(
                &task,
                Seq::new(3).expect("edit"),
                Seq::new(1).expect("claim"),
            )),
        );
    }

    #[test]
    fn task_contract_control_round_trips_exact_notice_authority() {
        let authority = NoticeAuthority {
            task: "1kKRRsDP".parse().expect("task stamp"),
            edit: Seq::new(2).expect("edit"),
            claim: Seq::new(1).expect("claim"),
        };
        let encoded = control(&authority.task, authority.edit, authority.claim);

        assert_eq!(notice_authority(Some(&encoded)), Ok(Some(authority)));
        assert_eq!(notice_authority(Some("lane-renewal-context")), Ok(None));
        assert!(notice_authority(Some("task-contract-edit:broken")).is_err());
    }

    #[test]
    fn startup_reconciliation_skips_collapsed_notice_without_a_journal_write() {
        let fixture = AdvisoryRtkFixture::new();
        let (bus, task) = claimed_fixture(&fixture);
        append_edit_with_summaries(
            &bus,
            &task,
            "updated contract",
            fixture.lane,
            Some(("the collapsed summary", "the collapsed summary")),
        );
        let before = bus.cursor().expect("pre-reconciliation cursor reads");
        let snapshot = bus.task_snapshot().expect("collapsed snapshot reads");

        assert!(
            !reconcile(
                &bus,
                &fixture.paths,
                fixture.epoch,
                &snapshot,
                None,
                Duration::from_secs(3_600),
            )
            .expect("collapsed startup notice is migration residue")
        );
        assert_eq!(
            bus.cursor().expect("post-reconciliation cursor reads"),
            before
        );
        let facts = bus.fact_snapshot().expect("collapsed facts read");
        let ledger = steer::Ledger::from_snapshot(&facts).expect("collapsed facts replay");
        assert!(ledger.pending(fixture.lane).is_empty());
        bus.verify().expect("collapsed recovery authority verifies");
        bus.shutdown().expect("collapsed recovery bus stops");
    }

    #[test]
    fn malformed_notice_shapes_still_fault() {
        let task: TaskStamp = "1kKRRsDP".parse().expect("task stamp");
        let edited = wire::body::TaskEdited {
            task: task.clone(),
            revision: 2,
            notice: None,
        };
        let valid_change = wire::body::TaskFieldEdit {
            field: "description".to_owned(),
            before: "before".to_owned(),
            after: "after".to_owned(),
        };
        let valid = wire::body::TaskEditNotice {
            handle: TaskHandle::for_project("board.test", &task),
            lane: CanonicalUuid::new(uuid::Uuid::from_u128(1)),
            claim: Seq::new(1).expect("claim position"),
            previous: Seq::new(1).expect("previous position"),
            changes: vec![valid_change.clone()],
        };
        let mut empty = valid.clone();
        empty.changes.clear();
        let mut unknown = valid.clone();
        unknown.changes[0].field = "unknown".to_owned();
        let mut unordered = valid.clone();
        unordered.changes = vec![
            valid_change,
            wire::body::TaskFieldEdit {
                field: "title".to_owned(),
                before: "before".to_owned(),
                after: "after".to_owned(),
            },
        ];
        let mut over_bound = valid;
        over_bound.changes[0].after = "x".repeat(wire::body::TASK_EDIT_FIELD_SUMMARY_CHARS + 1);

        for notice in [empty, unknown, unordered, over_bound] {
            let Err(Outcome::Fault(fault)) =
                admit_notice(&edited, Seq::new(2).expect("edit position"), &notice)
            else {
                panic!("malformed notice did not fault: {notice:?}");
            };
            assert_eq!(fault.reason, "task-contract::edit-invalid");
        }
    }

    #[test]
    fn claimed_edits_publish_once_in_order_without_moving_claim_or_lease_heads() {
        let fixture = AdvisoryRtkFixture::new();
        let (bus, task) = claimed_fixture(&fixture);
        let before = bus.task_snapshot().expect("claimed snapshot reads").tasks()[0].clone();
        append_edit(&bus, &task, "first contract", fixture.lane);
        append_edit(&bus, &task, "second contract", fixture.lane);
        let snapshot = bus.task_snapshot().expect("edited snapshot reads");

        assert!(
            reconcile(
                &bus,
                &fixture.paths,
                fixture.epoch,
                &snapshot,
                None,
                Duration::from_secs(3_600),
            )
            .expect("first edit notice reconciles")
        );
        assert!(
            !reconcile(
                &bus,
                &fixture.paths,
                fixture.epoch,
                &snapshot,
                None,
                Duration::from_secs(3_600),
            )
            .expect("repeated reconciliation is idempotent")
        );

        let facts = bus.fact_snapshot().expect("steering facts read");
        let ledger = steer::Ledger::from_snapshot(&facts).expect("steering facts replay");
        let pending = ledger.pending(fixture.lane);
        assert_eq!(pending.len(), 2);
        assert!(pending[0].text().contains("first contract"));
        assert!(pending[1].text().contains("second contract"));
        assert!(pending.iter().all(|directive| {
            directive
                .text()
                .contains("Acknowledge this revision before continuing.")
        }));
        let (authored, notices) =
            super::super::super::steering::pending_presentations(&ledger, fixture.lane)
                .expect("pending presentation partitions");
        assert!(authored.is_empty());
        assert_eq!(notices.len(), 2);
        assert_eq!(notices[0].task, task);
        let edit_positions = snapshot
            .facts()
            .iter()
            .filter(|envelope| envelope.fact.read::<wire::body::TaskEdited>().is_some())
            .map(|envelope| envelope.seq)
            .collect::<Vec<_>>();
        assert_eq!(
            notices.iter().map(|notice| notice.edit).collect::<Vec<_>>(),
            edit_positions
        );
        assert_eq!(
            notices[0].claim,
            before.claim().expect("claim head").body().claimed
        );
        let inbox = fixture
            .paths
            .inbox_for_worktree(fixture.root.to_str().expect("fixture path is UTF-8"))
            .expect("fixture inbox resolves");
        assert_eq!(
            fs::read_to_string(steer::inbox::path(&inbox, fixture.lane))
                .expect("disposable inbox reads"),
            "",
            "system notices never enter the authored steering rendering",
        );

        let after = bus.task_snapshot().expect("post-notice snapshot reads");
        let after = &after.tasks()[0];
        assert_eq!(after.claim(), before.claim());
        assert_eq!(after.lease(), before.lease());
        bus.verify().expect("contract edit authority verifies");
        bus.shutdown().expect("contract edit bus stops");
    }

    #[test]
    fn a_released_claim_cannot_receive_an_unreconciled_edit() {
        let fixture = AdvisoryRtkFixture::new();
        let (bus, task) = claimed_fixture(&fixture);
        append_edit(&bus, &task, "must not reach a successor", fixture.lane);
        release_claim(&bus, &task);
        let snapshot = bus.task_snapshot().expect("released snapshot reads");
        assert!(
            !reconcile(
                &bus,
                &fixture.paths,
                fixture.epoch,
                &snapshot,
                None,
                Duration::from_secs(3_600),
            )
            .expect("released edit reconciliation succeeds")
        );
        let facts = bus.fact_snapshot().expect("released facts read");
        let ledger = steer::Ledger::from_snapshot(&facts).expect("released facts replay");
        assert!(ledger.pending(fixture.lane).is_empty());
        bus.verify().expect("released authority verifies");
        bus.shutdown().expect("released bus stops");
    }

    #[test]
    fn claim_carry_preserves_the_pending_contract_notice_generation() {
        let fixture = AdvisoryRtkFixture::new();
        let (bus, task) = claimed_fixture(&fixture);
        append_edit(&bus, &task, "carried contract", fixture.lane);
        carry_claim(&bus, &task);
        let snapshot = bus.task_snapshot().expect("carried snapshot reads");
        assert!(
            reconcile(
                &bus,
                &fixture.paths,
                fixture.epoch,
                &snapshot,
                None,
                Duration::from_secs(3_600),
            )
            .expect("carried edit reconciliation succeeds")
        );
        let facts = bus.fact_snapshot().expect("carried facts read");
        let ledger = steer::Ledger::from_snapshot(&facts).expect("carried facts replay");
        assert_eq!(ledger.pending(fixture.lane).len(), 1);
        assert!(
            ledger.pending(fixture.lane)[0]
                .text()
                .contains("carried contract")
        );
        bus.verify().expect("carried authority verifies");
        bus.shutdown().expect("carried bus stops");
    }

    #[test]
    fn task_notes_never_open_claimant_contract_steering() {
        let fixture = AdvisoryRtkFixture::new();
        let (bus, task) = claimed_fixture(&fixture);
        let mut write = JournalWrite::new(
            Actor::Operator,
            format!("task:{task}").parse().expect("task origin"),
            now(),
            Producer::CommandDoor,
        );
        write
            .fact(
                wire::body::TaskNoted {
                    task,
                    note: "durable context only".to_owned(),
                },
                FactCause::Root,
            )
            .expect("task note builds");
        committed(bus.commit(write).expect("task note commits"));
        let snapshot = bus.task_snapshot().expect("noted snapshot reads");
        assert!(
            !reconcile(
                &bus,
                &fixture.paths,
                fixture.epoch,
                &snapshot,
                None,
                Duration::from_secs(3_600),
            )
            .expect("task note reconciliation succeeds")
        );
        let facts = bus.fact_snapshot().expect("noted facts read");
        let ledger = steer::Ledger::from_snapshot(&facts).expect("noted facts replay");
        assert!(ledger.pending(fixture.lane).is_empty());
        bus.verify().expect("noted authority verifies");
        bus.shutdown().expect("noted bus stops");
    }

    fn claimed_fixture(fixture: &AdvisoryRtkFixture) -> (Bus, TaskStamp) {
        let bus = Bus::start(Journal::in_memory().expect("fixture journal opens"))
            .expect("fixture bus starts");
        let row = LaneResidency {
            lane: fixture.lane,
            lineage_id: fixture.lane,
            name: Some("claimant".to_owned()),
            worktree: fixture.root.display().to_string(),
            life: "drive".to_owned(),
            vendor: fixture.vendor.clone(),
            occupant: Some(fixture.occupant),
            lineage: vec![fixture.occupant],
            capabilities: vec![fixture.capability],
            session: None,
            disposition: "occupied".to_owned(),
        };
        let origin: Origin = format!("resident:{}", fixture.epoch)
            .parse()
            .expect("resident origin");
        let now = now();
        let mut write = JournalWrite::new(Actor::Spice, origin, now, Producer::CommandDoor);
        let seated = write
            .fact(
                wire::body::UpSeated {
                    epoch: fixture.epoch,
                },
                FactCause::Root,
            )
            .expect("resident seating fact builds");
        let epoch = fixture.epoch;
        write.residency_head(seated, move |_| wire::ResidencyHead {
            epoch,
            state: ResidentState::Seated,
            topology_revision: 1,
            lanes: vec![row],
            subscriptions: Vec::new(),
            renewals: Vec::new(),
        });
        committed(bus.commit(write).expect("resident seating commits"));
        let task: TaskStamp = "1kKRRsDQ".parse().expect("task stamp");
        let start = shifted(now, -60);
        seed_task_claim(
            &bus,
            &task,
            &LaneResidency {
                lane: fixture.lane,
                lineage_id: fixture.lane,
                name: Some("claimant".to_owned()),
                worktree: fixture.root.display().to_string(),
                life: "drive".to_owned(),
                vendor: fixture.vendor.clone(),
                occupant: Some(fixture.occupant),
                lineage: vec![fixture.occupant],
                capabilities: vec![fixture.capability],
                session: None,
                disposition: "occupied".to_owned(),
            },
            fixture.epoch,
            start,
            shifted(now, 3_600),
            shifted(now, 7_200),
        );
        (bus, task)
    }

    fn append_edit(bus: &Bus, task: &TaskStamp, title: &str, lane: CanonicalUuid) {
        append_edit_with_summaries(bus, task, title, lane, None);
    }

    fn append_edit_with_summaries(
        bus: &Bus,
        task: &TaskStamp,
        title: &str,
        lane: CanonicalUuid,
        summaries: Option<(&str, &str)>,
    ) {
        let snapshot = bus.task_snapshot().expect("pre-edit snapshot reads");
        let heads = &snapshot.tasks()[0];
        let previous = heads.statement().body().clone();
        let claim = heads.claim().expect("active claim").body().claimed;
        let mut next = previous.clone();
        next.title = title.to_owned();
        let handle = TaskHandle::for_project(&next.project, task);
        let revision = u32::try_from(previous.revision.get())
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        let origin: Origin = format!("task:{task}").parse().expect("task origin");
        let (before, after) = summaries.map_or_else(
            || (previous.title.clone(), title.to_owned()),
            |(before, after)| (before.to_owned(), after.to_owned()),
        );
        let mut write = JournalWrite::new(Actor::Operator, origin, now(), Producer::CommandDoor);
        let edited = write
            .fact(
                wire::body::TaskEdited {
                    task: task.clone(),
                    revision,
                    notice: Some(wire::body::TaskEditNotice {
                        handle,
                        lane,
                        claim,
                        previous: previous.revision,
                        changes: vec![wire::body::TaskFieldEdit {
                            field: "title".to_owned(),
                            before,
                            after,
                        }],
                    }),
                },
                FactCause::Root,
            )
            .expect("edit fact builds");
        write.task_head(edited, move |position| TaskHead {
            revision: position,
            ..next
        });
        committed(bus.commit(write).expect("task edit commits"));
    }

    fn release_claim(bus: &Bus, task: &TaskStamp) {
        let snapshot = bus.task_snapshot().expect("pre-release snapshot reads");
        let claim = snapshot.tasks()[0]
            .claim()
            .expect("claim head")
            .body()
            .clone();
        let mut write = JournalWrite::new(
            claim.actor.clone(),
            format!("task:{task}").parse().expect("task origin"),
            now(),
            Producer::CommandDoor,
        );
        let freed = write
            .fact(
                wire::body::ClaimFreed {
                    task: task.clone(),
                    actor: claim.actor.clone(),
                },
                FactCause::Existing(claim.claimed),
            )
            .expect("claim release fact builds");
        write.claim_head(task.clone(), freed, move |position| wire::ClaimHead {
            fact: position,
            disposition: ClaimDisposition::Freed,
            ..claim
        });
        committed(bus.commit(write).expect("claim release commits"));
    }

    fn carry_claim(bus: &Bus, task: &TaskStamp) {
        let snapshot = bus.task_snapshot().expect("pre-carry snapshot reads");
        let claim = snapshot.tasks()[0]
            .claim()
            .expect("claim head")
            .body()
            .clone();
        let mut write = JournalWrite::new(
            Actor::Spice,
            format!("task:{task}").parse().expect("task origin"),
            now(),
            Producer::ResidentRecognition,
        );
        let carried = write
            .fact(
                wire::body::ClaimCarried {
                    task: task.clone(),
                    lane: claim.lane,
                    occupant: claim.occupant,
                    ancestor: claim.occupant,
                    epoch: claim.epoch,
                    source: wire::body::SourceBoundary::unrecorded(),
                },
                FactCause::Existing(claim.claimed),
            )
            .expect("claim carry fact builds");
        write.claim_head(task.clone(), carried, move |position| wire::ClaimHead {
            fact: position,
            ..claim
        });
        committed(bus.commit(write).expect("claim carry commits"));
    }

    fn now() -> wire::Instant {
        wire::Instant::new(jiff::Timestamp::now()).expect("fixture instant")
    }

    fn shifted(at: wire::Instant, seconds: i64) -> wire::Instant {
        wire::Instant::new(
            at.get()
                .checked_add(jiff::SignedDuration::from_secs(seconds))
                .expect("fixture instant shifts"),
        )
        .expect("shifted fixture instant")
    }

    fn committed(outcome: CommitOutcome) {
        assert!(matches!(outcome, CommitOutcome::Committed(_)));
    }
}
