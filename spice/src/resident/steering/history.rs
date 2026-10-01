//! Operator direction history: one bounded page of fanout groups.
//!
//! The journal projects every operator-authored directive without its text;
//! grouping, filtering, and windowing happen here so a fanout is kept or
//! dropped whole, and only the returned page's texts are read.

use std::collections::BTreeMap;
use std::ops::Range;

use bus::Bus;
use wire::intent::{
    CommandAnswer, DIRECTION_HISTORY_LANES_MAX, DIRECTION_HISTORY_PAGE_LIMIT_MAX,
    DIRECTION_HISTORY_QUERY_BYTES_MAX, DirectionGroup, DirectionHistoryAnswer, DirectionRecipient,
    DirectionRetirement, DirectionWindow, SteerHistory,
};
use wire::{CanonicalUuid, Outcome, Reply, ReplyPosition, Seq};

use super::{RuntimePaths, directive_preview, refusal, typed_store_fault};

/// Directives of one operator submit action, in creation order.
struct Group {
    fanout: CanonicalUuid,
    members: Vec<journal::OperatorDirection>,
}

impl Group {
    fn first(&self) -> &journal::OperatorDirection {
        self.members
            .first()
            .expect("a direction group holds at least one directive")
    }

    fn position(&self) -> Seq {
        self.first().created
    }

    fn holds(&self, identity: CanonicalUuid) -> bool {
        self.fanout == identity
            || self
                .members
                .iter()
                .any(|member| member.directive == identity)
    }

    fn holds_key(&self, key: &str) -> bool {
        self.members.iter().any(|member| member.key == key)
    }
}

pub(in crate::resident) fn history(
    bus: &Bus,
    paths: &RuntimePaths,
    request: &SteerHistory,
) -> Outcome<CommandAnswer> {
    let query = match admitted_query(request) {
        Ok(query) => query,
        Err(outcome) => return outcome,
    };
    let reader = bus.reader();
    let directions = match reader.operator_directions(query) {
        Ok(directions) => directions,
        Err(error) => return typed_store_fault("steer::index-unreadable", &error.to_string()),
    };
    let Some(position) = directions.horizon else {
        return typed_store_fault(
            "steer::journal-empty",
            "a running resident has no durable startup fact",
        );
    };
    let groups = match grouped(directions.rows) {
        Ok(groups) => groups,
        Err(outcome) => return outcome,
    };
    let matching: Vec<Group> = groups
        .into_iter()
        .filter(|group| admits(request, group))
        .collect();
    let range = match window(&request.window, &matching, usize::from(request.limit)) {
        Ok(range) => range,
        Err(outcome) => return outcome,
    };
    let older = (!range.is_empty() && range.end < matching.len())
        .then(|| matching[range.end - 1].position());
    let newer = (!range.is_empty() && range.start > 0).then(|| matching[range.start].position());
    let total = u64::try_from(matching.len()).expect("direction group count fits the wire total");
    let offset = u64::try_from(range.start).expect("direction group index fits the wire offset");
    let page = &matching[range];
    let texts = match reader
        .operator_direction_texts(page.iter().map(|group| group.first().directive).collect())
    {
        Ok(texts) => texts,
        Err(error) => return typed_store_fault("steer::index-unreadable", &error.to_string()),
    };
    let resolver = spice::quote::Resolver::from_tape_with_agent_presenter(
        &paths.tape,
        &reader,
        super::super::quote_agent_prose,
    );
    let mut rows = Vec::with_capacity(page.len());
    for (group, text) in page.iter().zip(texts) {
        let Some(text) = text else {
            return typed_store_fault(
                "steer::history-text-missing",
                &format!("directive {} has no indexed text", group.first().directive),
            );
        };
        rows.push(presented(group, directive_preview(&resolver.expand(&text))));
    }
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(position),
        body: CommandAnswer::DirectionHistory(Box::new(DirectionHistoryAnswer {
            groups: rows,
            limit: request.limit,
            total,
            offset,
            older,
            newer,
        })),
    })
}

/// The ASCII-lowercased query, absent when unfiltered, after every bound.
fn admitted_query(request: &SteerHistory) -> Result<Option<String>, Outcome<CommandAnswer>> {
    if request.limit == 0 || request.limit > DIRECTION_HISTORY_PAGE_LIMIT_MAX {
        return Err(refusal(
            "steer::history-limit-invalid",
            Some(format!("1..={DIRECTION_HISTORY_PAGE_LIMIT_MAX}")),
            Some(request.limit.to_string()),
            "request a bounded direction history page",
        ));
    }
    if request.lanes.len() > DIRECTION_HISTORY_LANES_MAX {
        return Err(refusal(
            "steer::history-lanes-invalid",
            Some(format!("at most {DIRECTION_HISTORY_LANES_MAX} lanes")),
            Some(request.lanes.len().to_string()),
            "filter direction history by fewer lanes",
        ));
    }
    let Some(query) = request.query.as_deref() else {
        return Ok(None);
    };
    if query.len() > DIRECTION_HISTORY_QUERY_BYTES_MAX {
        return Err(refusal(
            "steer::history-query-invalid",
            Some(format!("at most {DIRECTION_HISTORY_QUERY_BYTES_MAX} bytes")),
            Some(format!("{} bytes", query.len())),
            "shorten the direction history query",
        ));
    }
    let query = query.trim();
    if query.chars().any(char::is_control) {
        return Err(refusal(
            "steer::history-query-invalid",
            Some("printable text".to_owned()),
            Some("a control character".to_owned()),
            "search direction history with printable text",
        ));
    }
    Ok((!query.is_empty()).then(|| query.to_ascii_lowercase()))
}

/// Fanout groups ordered newest first by their first creating fact.
fn grouped(rows: Vec<journal::OperatorDirection>) -> Result<Vec<Group>, Outcome<CommandAnswer>> {
    let mut groups: BTreeMap<CanonicalUuid, Vec<journal::OperatorDirection>> = BTreeMap::new();
    for row in rows {
        let Some(fanout) = row.fanout else {
            return Err(typed_store_fault(
                "steer::history-fanout-missing",
                &format!(
                    "operator directive {} carries no fanout identity",
                    row.directive
                ),
            ));
        };
        groups.entry(fanout).or_default().push(row);
    }
    let mut groups: Vec<Group> = groups
        .into_iter()
        .map(|(fanout, mut members)| {
            members.sort_by_key(|member| member.created);
            Group { fanout, members }
        })
        .collect();
    groups.sort_by_key(|group| std::cmp::Reverse(group.position()));
    Ok(groups)
}

/// Whether every filter kind the request names keeps this whole group.
fn admits(request: &SteerHistory, group: &Group) -> bool {
    let members = &group.members;
    members.iter().any(|member| member.matched)
        && (request.lanes.is_empty()
            || members
                .iter()
                .any(|member| request.lanes.contains(&member.lane)))
        && (request.dispositions.is_empty()
            || members
                .iter()
                .any(|member| request.dispositions.contains(&member.disposition)))
        && request
            .authored_before
            .is_none_or(|cutoff| group.first().published <= cutoff)
}

/// Indices of the page within the newest-first matching groups.
fn window(
    window: &DirectionWindow,
    matching: &[Group],
    limit: usize,
) -> Result<Range<usize>, Outcome<CommandAnswer>> {
    let older_from = |start: usize| start..start.saturating_add(limit).min(matching.len());
    let anchored = |start: Option<usize>, identity: String| {
        start.map(older_from).ok_or_else(|| {
            refusal(
                "steer::history-anchor-unknown",
                Some("an operator direction matching the filters".to_owned()),
                Some(identity),
                "anchor on a directive, fanout, or key this filtered history holds",
            )
        })
    };
    match window {
        DirectionWindow::Newest => Ok(older_from(0)),
        DirectionWindow::Before { position } => Ok(older_from(
            matching.partition_point(|group| group.position() >= *position),
        )),
        DirectionWindow::After { position } => {
            let end = matching.partition_point(|group| group.position() > *position);
            Ok(end.saturating_sub(limit)..end)
        }
        DirectionWindow::Anchor { identity } => anchored(
            matching.iter().position(|group| group.holds(*identity)),
            identity.to_string(),
        ),
        DirectionWindow::AnchorKey { key } => anchored(
            matching.iter().position(|group| group.holds_key(key)),
            key.clone(),
        ),
    }
}

fn presented(group: &Group, preview: Option<String>) -> DirectionGroup {
    DirectionGroup {
        fanout: group.fanout,
        position: group.position(),
        preview,
        authored: group.first().published,
        recipients: group
            .members
            .iter()
            .map(|member| DirectionRecipient {
                directive: member.directive,
                lane: member.lane,
                key: member.key.clone(),
                disposition: member.disposition,
                retirement: member
                    .retired
                    .map(|(position, at)| DirectionRetirement { position, at }),
            })
            .collect(),
    }
}
