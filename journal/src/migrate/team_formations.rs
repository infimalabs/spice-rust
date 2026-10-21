use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OptionalExtension, params};

use super::{OpenError, TEAM_FORMATIONS_RELEASE, residual};

/// Recover every live team's latest unteamed-to-teamed instant, under the one
/// [`wire::head::teamed`] predicate, from immutable topology facts, then
/// persist exactly those instants in the residency authority.
pub(super) fn cross(connection: &Connection) -> Result<(), OpenError> {
    let Some((head_seq, body)) = connection
        .query_row(
            "SELECT head_seq, body FROM residency_head WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
    else {
        return Ok(());
    };
    let mut head = wire::decode::<wire::ResidencyHead>(&body)
        .map_err(|error| formation_error(format!("the stored head does not decode: {error}")))?;
    let mut memberships = BTreeMap::<wire::CanonicalUuid, BTreeSet<_>>::new();
    for lane in &head.lanes {
        memberships.entry(lane.team).or_default().insert(lane.lane);
    }
    let mut unresolved = memberships
        .iter()
        .filter_map(|(team, members)| {
            wire::head::teamed(*team, members.iter().copied()).then_some(*team)
        })
        .collect::<BTreeSet<_>>();
    let mut formations = BTreeMap::new();
    let mut query = connection
        .prepare(
            "SELECT envelope FROM fact_envelopes
             WHERE kind IN (
                'lane::seated',
                'lane::removed',
                'lane::arranged',
                'team::configured',
                'team::arranged'
             )
             ORDER BY seq DESC",
        )
        .map_err(OpenError::Sqlite)?;
    let facts = query
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .map_err(OpenError::Sqlite)?;
    for bytes in facts {
        if unresolved.is_empty() {
            break;
        }
        let bytes = bytes.map_err(OpenError::Sqlite)?;
        let envelope = wire::decode::<wire::Envelope<wire::AnyFact>>(&bytes).map_err(|error| {
            formation_error(format!("a topology fact does not decode: {error}"))
        })?;
        rewind_memberships(
            &envelope,
            &mut memberships,
            &mut unresolved,
            &mut formations,
        );
    }
    if !unresolved.is_empty() {
        return Err(formation_error(format!(
            "formation history is missing for live teams {}",
            unresolved
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    for subscription in &mut head.subscriptions {
        subscription.formed_at = formations.get(&subscription.team).copied();
    }
    let bytes = wire::encode(&head)
        .map_err(|error| formation_error(format!("the crossed head cannot encode: {error}")))?;
    let changed = connection
        .execute(
            "UPDATE residency_head SET body = ?1 WHERE singleton = 1 AND head_seq = ?2",
            params![bytes, head_seq],
        )
        .map_err(OpenError::Sqlite)?;
    if changed != 1 {
        return Err(formation_error(
            "the head changed while its exclusive migration held the authority",
        ));
    }
    Ok(())
}

fn rewind_memberships(
    envelope: &wire::Envelope<wire::AnyFact>,
    memberships: &mut BTreeMap<wire::CanonicalUuid, BTreeSet<wire::CanonicalUuid>>,
    unresolved: &mut BTreeSet<wire::CanonicalUuid>,
    formations: &mut BTreeMap<wire::CanonicalUuid, wire::Instant>,
) {
    unresolved.retain(|team| {
        let after = memberships.remove(team).unwrap_or_default();
        let was_formed = wire::head::teamed(*team, after.iter().copied());
        let before = membership_before(*team, after, &envelope.fact);
        let formed_here = was_formed && !wire::head::teamed(*team, before.iter().copied());
        memberships.insert(*team, before);
        if formed_here {
            formations.insert(*team, envelope.at);
        }
        !formed_here
    });
}

fn membership_before(
    team: wire::CanonicalUuid,
    mut before: BTreeSet<wire::CanonicalUuid>,
    fact: &wire::AnyFact,
) -> BTreeSet<wire::CanonicalUuid> {
    if let Some(body) = fact.read::<wire::body::LaneSeated>() {
        if body.team == team {
            before.remove(&body.lane);
        }
    } else if let Some(body) = fact.read::<wire::body::LaneRemoved>() {
        if body.team == team {
            before.insert(body.lane);
        }
    } else if let Some(body) = fact.read::<wire::body::LaneArranged>() {
        if body.team == team {
            before.remove(&body.lane);
        }
        if body.from_team == team {
            before.insert(body.lane);
        }
    } else if let Some(body) = fact.read::<wire::body::TeamConfigured>() {
        if body.source_rekeyed_to == Some(team) {
            before.clear();
        } else if let Some(member) = body.member {
            if body.team == team {
                before.remove(&member);
            }
            if body.from_team == Some(team) {
                before.insert(member);
            }
        }
    } else if let Some(body) = fact.read::<wire::body::TeamArranged>() {
        before = body
            .from
            .iter()
            .find(|standing| standing.team == team)
            .map(|standing| standing.lanes.iter().copied().collect())
            .unwrap_or_default();
    }
    before
}

fn formation_error(detail: impl Into<String>) -> OpenError {
    residual::retained(
        "residency_head",
        "1".to_owned(),
        TEAM_FORMATIONS_RELEASE,
        detail.into(),
    )
}
