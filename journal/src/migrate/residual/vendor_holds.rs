//! Vendor-failure holds recorded before a hold carried the input it refused.
//!
//! The refused input is what the resident would have resumed at the hold: the
//! held lane's declared session, the operator directives still live on that
//! lane, and the lane's renewal revision. Each is a fold of authority facts, so
//! the crossing derives every retained hold's input from the facts strictly
//! preceding it, and refuses a hold whose history does not prove that input.

use std::collections::{BTreeMap, btree_map::Entry};

use rusqlite::{Transaction, params_from_iter, types::Value as Sql};
use wire::{CanonicalUuid, FactKind, body::VendorFailureInput};

use super::{OpenError, VENDOR_FAILURE_INPUT_RELEASE, retained, rewrite_facts};

/// Selects a vendor-failure hold that omits its refused input.
const RETAINED: &str = "json_type(CAST(envelope AS TEXT), '$.fact.vendor_failure') IS NULL \
     AND instr(COALESCE(json_extract(CAST(envelope AS TEXT), '$.fact.evidence'), ''), \
     ' ended on a vendor error:') > 0";

/// Kinds that move a lane's row, seat, session, or renewal.
const LANE_KINDS: [FactKind; 7] = [
    FactKind::LaneSeated,
    FactKind::LaneRemoved,
    FactKind::LaneSessionDeclared,
    FactKind::LaneStarted,
    FactKind::LaneRenewed,
    FactKind::LaneRenewalChanged,
    FactKind::UpSeated,
];

/// Every retained hold's derived input, keyed by the hold's position.
pub(in crate::migrate) struct Derived(BTreeMap<i64, VendorFailureInput>);

/// Derive every retained hold's input without mutating anything, refusing the
/// first hold whose preceding history does not prove it.
pub(super) fn derive(transaction: &Transaction<'_>) -> Result<Derived, OpenError> {
    let held = retained_holds(transaction)?;
    let Some(&last) = held.last() else {
        return Ok(Derived(BTreeMap::new()));
    };
    let kinds = FactKind::ALL
        .iter()
        .copied()
        .filter(|kind| {
            *kind == FactKind::LaneHeld
                || LANE_KINDS.contains(kind)
                || crate::guard::steering_disposition(*kind).is_some()
        })
        .map(|kind| Sql::Text(kind.as_str().to_owned()))
        .collect::<Vec<_>>();
    let placeholders = (2..2 + kinds.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = transaction
        .prepare(&format!(
            "SELECT seq, envelope FROM fact_envelopes \
             WHERE seq <= ?1 AND kind IN ({placeholders}) ORDER BY seq"
        ))
        .map_err(OpenError::Sqlite)?;
    let mut rows = statement
        .query(params_from_iter(
            std::iter::once(Sql::Integer(last)).chain(kinds),
        ))
        .map_err(OpenError::Sqlite)?;
    let mut history = History::default();
    let mut derived = BTreeMap::new();
    while let Some(row) = rows.next().map_err(OpenError::Sqlite)? {
        let seq = row.get::<_, i64>(0).map_err(OpenError::Sqlite)?;
        let bytes = row.get::<_, Vec<u8>>(1).map_err(OpenError::Sqlite)?;
        let envelope = wire::decode::<wire::Envelope<wire::AnyFact>>(&bytes).map_err(|error| {
            refusal(
                seq,
                format!("a fact preceding a vendor-failure hold does not decode: {error}"),
            )
        })?;
        if held.binary_search(&seq).is_ok() {
            let input = history
                .input(&envelope.fact)
                .map_err(|detail| refusal(seq, detail))?;
            derived.insert(seq, input);
        } else {
            history.remember(seq, &envelope);
        }
    }
    Ok(Derived(derived))
}

/// Give every retained hold the input [`derive`] proved for it.
pub(super) fn cross(transaction: &Transaction<'_>, derived: Derived) -> Result<(), OpenError> {
    let Derived(inputs) = derived;
    rewrite_facts(
        transaction,
        FactKind::LaneHeld,
        RETAINED,
        VENDOR_FAILURE_INPUT_RELEASE,
        |seq, fact| {
            let input = inputs
                .get(&seq)
                .ok_or_else(|| "the hold was not derived before the crossing".to_owned())?;
            let value = serde_json::to_value(input).map_err(|error| error.to_string())?;
            fact.insert("vendor_failure".to_owned(), value);
            Ok(())
        },
    )
}

fn retained_holds(transaction: &Transaction<'_>) -> Result<Vec<i64>, OpenError> {
    let mut statement = transaction
        .prepare(&format!(
            "SELECT seq FROM fact_envelopes WHERE kind = ?1 AND ({RETAINED}) ORDER BY seq"
        ))
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([FactKind::LaneHeld.as_str()], |row| row.get::<_, i64>(0))
        .map_err(OpenError::Sqlite)?;
    rows.collect::<Result<_, _>>().map_err(OpenError::Sqlite)
}

fn refusal(seq: i64, detail: String) -> OpenError {
    retained(
        "fact_envelopes",
        seq.to_string(),
        VENDOR_FAILURE_INPUT_RELEASE,
        detail,
    )
}

/// The occupant a vendor-failure hold's evidence names as ended.
fn ended_occupant(evidence: &str) -> Option<CanonicalUuid> {
    let (occupant, _) = evidence
        .strip_prefix("occupant ")?
        .split_once(" ended on a vendor error:")?;
    occupant.parse().ok()
}

/// Lane and steering standing as of the facts folded so far.
#[derive(Default)]
struct History {
    lanes: BTreeMap<CanonicalUuid, Lane>,
    directives: BTreeMap<CanonicalUuid, Directive>,
    keys: BTreeMap<String, CanonicalUuid>,
    /// Lanes whose live directives history cannot prove, with the first reason.
    unproved: BTreeMap<CanonicalUuid, String>,
}

#[derive(Default)]
struct Lane {
    removed: bool,
    declared: Option<Declared>,
    seat: Option<Seat>,
    renewal: Option<u64>,
}

struct Declared {
    seq: i64,
    occupant: CanonicalUuid,
    identity: String,
}

#[derive(Clone, Copy)]
struct Seat {
    seq: i64,
    occupant: CanonicalUuid,
}

struct Directive {
    lane: CanonicalUuid,
    created: i64,
    operator: bool,
    live: bool,
}

impl History {
    fn remember(&mut self, seq: i64, envelope: &wire::Envelope<wire::AnyFact>) {
        let fact = &envelope.fact;
        if let Some(body) = fact.read::<wire::body::LaneSeated>() {
            self.lanes.insert(body.lane, Lane::default());
        } else if let Some(body) = fact.read::<wire::body::LaneRemoved>() {
            let removed = Lane {
                removed: true,
                ..Lane::default()
            };
            self.lanes.insert(body.lane, removed);
        } else if let Some(body) = fact.read::<wire::body::LaneSessionDeclared>() {
            self.lanes.entry(body.lane).or_default().declared = Some(Declared {
                seq,
                occupant: body.occupant,
                identity: body.identity,
            });
        } else if let Some(body) = fact.read::<wire::body::LaneStarted>() {
            self.seat(body.lane, seq, body.occupant);
        } else if let Some(body) = fact.read::<wire::body::LaneRenewed>() {
            self.seat(body.lane, seq, body.occupant);
        } else if let Some(body) = fact.read::<wire::body::LaneRenewalChanged>() {
            self.lanes.entry(body.lane).or_default().renewal =
                body.renewal.map(|renewal| renewal.revision);
        } else if fact.read::<wire::body::UpSeated>().is_some() {
            // Each resident start seats a residency head with no renewals.
            for lane in self.lanes.values_mut() {
                lane.renewal = None;
            }
        } else {
            self.steer(seq, envelope);
        }
    }

    fn seat(&mut self, lane: CanonicalUuid, seq: i64, occupant: CanonicalUuid) {
        self.lanes.entry(lane).or_default().seat = Some(Seat { seq, occupant });
    }

    fn steer(&mut self, seq: i64, envelope: &wire::Envelope<wire::AnyFact>) {
        let (Some(identity), Some(disposition)) = (
            crate::guard::steering_fact_identity(&envelope.fact),
            crate::guard::steering_disposition(envelope.fact.kind()),
        ) else {
            return;
        };
        let lane = identity.lane;
        let Some(directive) = identity
            .directive
            .or_else(|| self.keys.get(&identity.key).copied())
        else {
            self.unprove(
                lane,
                format!(
                    "steering fact {seq} names key {} that no earlier directive published",
                    identity.key
                ),
            );
            return;
        };
        if *self.keys.entry(identity.key).or_insert(directive) != directive {
            self.unprove(
                lane,
                format!("steering fact {seq} moves its key to another directive"),
            );
            return;
        }
        let live = disposition == wire::Disposition::Live;
        match self.directives.entry(directive) {
            Entry::Vacant(entry) => {
                entry.insert(Directive {
                    lane,
                    created: seq,
                    operator: wire::principal::ActingPrincipal::of(&envelope.actor).is_some(),
                    live,
                });
            }
            Entry::Occupied(entry) if entry.get().lane != lane => {
                let created = entry.get().lane;
                self.unprove(
                    created,
                    format!("steering fact {seq} moves its directive to another lane"),
                );
                self.unprove(
                    lane,
                    format!("steering fact {seq} moves its directive to another lane"),
                );
            }
            Entry::Occupied(mut entry) => entry.get_mut().live = live,
        }
    }

    fn unprove(&mut self, lane: CanonicalUuid, detail: String) {
        self.unproved.entry(lane).or_insert(detail);
    }

    /// The input a hold refused, from the history folded before it.
    fn input(&self, fact: &wire::AnyFact) -> Result<VendorFailureInput, String> {
        let held = fact
            .read::<wire::body::LaneHeld>()
            .ok_or_else(|| "the selected row is not a lane hold".to_owned())?;
        let occupant = ended_occupant(&held.evidence)
            .ok_or_else(|| "the hold evidence names no ended occupant".to_owned())?;
        if let Some(detail) = self.unproved.get(&held.lane) {
            return Err(format!(
                "the held lane's live directives are unproved: {detail}"
            ));
        }
        let lane = self
            .lanes
            .get(&held.lane)
            .filter(|lane| !lane.removed)
            .ok_or_else(|| "the held lane has no residency row before the hold".to_owned())?;
        let seat = lane
            .seat
            .filter(|seat| seat.occupant == occupant)
            .ok_or_else(|| format!("occupant {occupant} is not the held lane's latest seat"))?;
        let declared = lane
            .declared
            .as_ref()
            .filter(|declared| declared.occupant == occupant && declared.seq < seat.seq)
            .ok_or_else(|| {
                format!("occupant {occupant} was seated without a session declaration")
            })?;
        let mut live = self
            .directives
            .iter()
            .filter(|(_, directive)| {
                directive.lane == held.lane && directive.operator && directive.live
            })
            .map(|(identity, directive)| (directive.created, *identity))
            .collect::<Vec<_>>();
        live.sort_unstable();
        Ok(VendorFailureInput {
            session: Some(declared.identity.clone()),
            directives: live.into_iter().map(|(_, identity)| identity).collect(),
            renewal: lane.renewal,
        })
    }
}
