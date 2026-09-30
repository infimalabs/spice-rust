//! Stable projection of browser submissions and resident directive lifecycles.

use std::collections::{BTreeMap, BTreeSet};

use wire::CanonicalUuid;

/// Browser submission projected into the team directive ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectiveLedgerLocal {
    pub fanout: CanonicalUuid,
    pub lane: CanonicalUuid,
    pub authored_millis: i64,
    pub body: String,
    pub attachments: Vec<wire::body::DirectiveAttachment>,
    pub key: Option<String>,
    pub delivered: bool,
}

/// One recipient's current directive lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DirectiveLedgerState {
    Queued,
    Delivered,
    Acknowledged,
    Declined,
    DeadLettered,
    Lapsed,
    Canceled,
}

impl DirectiveLedgerState {
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Delivered => "delivered",
            Self::Acknowledged => "acknowledged",
            Self::Declined => "declined",
            Self::DeadLettered => "dead-lettered",
            Self::Lapsed => "lapsed",
            Self::Canceled => "canceled",
        }
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Delivered => "Delivered",
            Self::Acknowledged => "Acknowledged",
            Self::Declined => "Declined",
            Self::DeadLettered => "Dead-lettered",
            Self::Lapsed => "Lapsed",
            Self::Canceled => "Canceled",
        }
    }

    const fn from_disposition(disposition: wire::Disposition) -> Self {
        match disposition {
            wire::Disposition::Live => Self::Delivered,
            wire::Disposition::Acked => Self::Acknowledged,
            wire::Disposition::Declined => Self::Declined,
            wire::Disposition::DeadLettered => Self::DeadLettered,
            wire::Disposition::Lapsed => Self::Lapsed,
            wire::Disposition::Canceled => Self::Canceled,
        }
    }
}

/// One stable fanout or directive row across local, live, and retired state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectiveLedgerRow {
    pub identity: CanonicalUuid,
    pub authored_millis: i64,
    pub body: String,
    pub body_complete: bool,
    pub attachments: Vec<wire::body::DirectiveAttachment>,
    /// A peer lane that authored the direction; absent means the operator.
    pub author: Option<CanonicalUuid>,
    pub recipients: BTreeMap<CanonicalUuid, DirectiveLedgerState>,
    pub recipient_keys: BTreeMap<CanonicalUuid, String>,
    pub keys: BTreeSet<String>,
}

/// Join browser submissions and resident lifecycle records for one team's lanes.
pub(crate) fn directive_ledger(
    members: &[CanonicalUuid],
    local: &[DirectiveLedgerLocal],
    inbox: Option<&wire::intent::InboxAnswer>,
) -> Vec<DirectiveLedgerRow> {
    let member_lanes = members.iter().copied().collect::<BTreeSet<_>>();
    let mut rows = BTreeMap::<CanonicalUuid, DirectiveLedgerRow>::new();
    for submission in local
        .iter()
        .filter(|submission| member_lanes.contains(&submission.lane))
    {
        let row = rows
            .entry(submission.fanout)
            .or_insert_with(|| DirectiveLedgerRow {
                identity: submission.fanout,
                authored_millis: submission.authored_millis,
                body: submission.body.clone(),
                body_complete: true,
                attachments: submission.attachments.clone(),
                author: None,
                recipients: BTreeMap::new(),
                recipient_keys: BTreeMap::new(),
                keys: BTreeSet::new(),
            });
        row.authored_millis = row.authored_millis.min(submission.authored_millis);
        row.recipients.insert(
            submission.lane,
            if submission.delivered {
                DirectiveLedgerState::Delivered
            } else {
                DirectiveLedgerState::Queued
            },
        );
        row.keys.extend(submission.key.iter().cloned());
        if let Some(key) = &submission.key {
            row.recipient_keys.insert(submission.lane, key.clone());
        }
    }
    if let Some(inbox) = inbox {
        for lane in inbox
            .lanes
            .iter()
            .filter(|lane| member_lanes.contains(&lane.lane))
        {
            for directive in &lane.pending {
                let identity = directive.fanout.unwrap_or(directive.directive);
                let row = rows.entry(identity).or_insert_with(|| DirectiveLedgerRow {
                    identity,
                    authored_millis: directive.authored.get().as_millisecond(),
                    body: directive.text.clone(),
                    body_complete: true,
                    attachments: directive.attachments.clone(),
                    author: directive.from,
                    recipients: BTreeMap::new(),
                    recipient_keys: BTreeMap::new(),
                    keys: BTreeSet::new(),
                });
                row.authored_millis = row
                    .authored_millis
                    .min(directive.authored.get().as_millisecond());
                row.author = row.author.or(directive.from);
                row.recipients
                    .insert(lane.lane, DirectiveLedgerState::Delivered);
                row.keys.insert(directive.key.clone());
                row.recipient_keys.insert(lane.lane, directive.key.clone());
            }
            for directive in &lane.retired {
                let identity = directive.fanout.unwrap_or(directive.directive);
                let row = rows.entry(identity).or_insert_with(|| DirectiveLedgerRow {
                    identity,
                    authored_millis: directive.authored.get().as_millisecond(),
                    body: directive
                        .preview
                        .clone()
                        .unwrap_or_else(|| "Preview unavailable".to_owned()),
                    body_complete: false,
                    attachments: Vec::new(),
                    author: directive.from,
                    recipients: BTreeMap::new(),
                    recipient_keys: BTreeMap::new(),
                    keys: BTreeSet::new(),
                });
                row.authored_millis = row
                    .authored_millis
                    .min(directive.authored.get().as_millisecond());
                row.author = row.author.or(directive.from);
                if row.body.is_empty() {
                    row.body = directive
                        .preview
                        .clone()
                        .unwrap_or_else(|| "Preview unavailable".to_owned());
                    row.body_complete = false;
                }
                row.recipients.insert(
                    lane.lane,
                    DirectiveLedgerState::from_disposition(directive.disposition),
                );
                row.keys.insert(directive.key.clone());
                row.recipient_keys.insert(lane.lane, directive.key.clone());
            }
        }
    }
    let mut rows = rows.into_values().collect::<Vec<_>>();
    rows.sort_by_key(|row| (std::cmp::Reverse(row.authored_millis), row.identity));
    rows
}

/// Stable row identity carrying one recipient's steering key.
pub(crate) fn direction_identity(
    members: &BTreeSet<CanonicalUuid>,
    inbox: &wire::intent::InboxAnswer,
    key: &str,
) -> Option<CanonicalUuid> {
    inbox
        .lanes
        .iter()
        .filter(|lane| members.contains(&lane.lane))
        .flat_map(|lane| {
            lane.pending
                .iter()
                .map(|directive| {
                    (
                        directive.key.as_str(),
                        directive.fanout.unwrap_or(directive.directive),
                    )
                })
                .chain(lane.retired.iter().map(|directive| {
                    (
                        directive.key.as_str(),
                        directive.fanout.unwrap_or(directive.directive),
                    )
                }))
        })
        .find_map(|(candidate, identity)| (candidate == key).then_some(identity))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{DirectiveLedgerLocal, DirectiveLedgerState, direction_identity, directive_ledger};

    fn id(suffix: u8) -> wire::CanonicalUuid {
        format!("00000000-0000-4000-8000-{suffix:012}")
            .parse()
            .expect("fixture identity is canonical")
    }

    #[test]
    fn a_fanout_keeps_one_row_while_each_recipient_advances_independently() {
        let authored = "2030-01-01T00:00:00.000Z".parse::<wire::Instant>().unwrap();
        let retired = "2030-01-01T00:01:00.000Z".parse::<wire::Instant>().unwrap();
        let fanout = id(9);
        let local = vec![
            DirectiveLedgerLocal {
                fanout,
                lane: id(1),
                authored_millis: 100,
                body: "one instruction".to_owned(),
                attachments: Vec::new(),
                key: None,
                delivered: false,
            },
            DirectiveLedgerLocal {
                fanout,
                lane: id(2),
                authored_millis: 100,
                body: "one instruction".to_owned(),
                attachments: Vec::new(),
                key: None,
                delivered: false,
            },
        ];
        let inbox = wire::intent::InboxAnswer {
            epoch: id(8),
            lanes: vec![
                wire::intent::LaneInbox {
                    lane: id(1),
                    occupant: None,
                    compaction: None,
                    pending: vec![wire::intent::PendingDirective {
                        directive: id(3),
                        key: "1kLive".to_owned(),
                        fanout: Some(fanout),
                        text: "one instruction".to_owned(),
                        from: None,
                        preview: Some("one instruction".to_owned()),
                        control: None,
                        attachments: Vec::new(),
                        authored,
                        stage: wire::body::SteeringStage::Published,
                        escalations: Vec::new(),
                        expires: retired,
                    }],
                    retired: Vec::new(),
                },
                wire::intent::LaneInbox {
                    lane: id(2),
                    occupant: None,
                    compaction: None,
                    pending: Vec::new(),
                    retired: vec![wire::intent::RetiredDirective {
                        directive: id(4),
                        key: "1kDone".to_owned(),
                        fanout: Some(fanout),
                        from: None,
                        preview: Some("one instruction".to_owned()),
                        authored,
                        disposition: wire::Disposition::Acked,
                        position: wire::Seq(std::num::NonZeroU64::new(1).unwrap()),
                        retired,
                    }],
                },
            ],
        };

        let rows = directive_ledger(&[id(1), id(2)], &local, Some(&inbox));

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].identity, fanout);
        assert_eq!(rows[0].authored_millis, 100);
        assert_eq!(rows[0].body, "one instruction");
        assert!(rows[0].body_complete);
        assert_eq!(
            rows[0].recipients,
            BTreeMap::from([
                (id(1), DirectiveLedgerState::Delivered),
                (id(2), DirectiveLedgerState::Acknowledged),
            ])
        );
        assert_eq!(
            rows[0].keys,
            BTreeSet::from(["1kDone".to_owned(), "1kLive".to_owned()])
        );
        assert_eq!(
            rows[0].recipient_keys,
            BTreeMap::from([(id(1), "1kLive".to_owned()), (id(2), "1kDone".to_owned()),])
        );
        assert_eq!(
            direction_identity(&BTreeSet::from([id(1), id(2)]), &inbox, "1kDone"),
            Some(fanout)
        );
    }

    #[test]
    fn newest_direction_is_first() {
        let local = [
            DirectiveLedgerLocal {
                fanout: id(8),
                lane: id(1),
                authored_millis: 100,
                body: "older".to_owned(),
                attachments: Vec::new(),
                key: None,
                delivered: false,
            },
            DirectiveLedgerLocal {
                fanout: id(9),
                lane: id(1),
                authored_millis: 200,
                body: "newer".to_owned(),
                attachments: Vec::new(),
                key: None,
                delivered: false,
            },
        ];

        let rows = directive_ledger(&[id(1)], &local, None);

        assert_eq!(
            rows.iter().map(|row| row.body.as_str()).collect::<Vec<_>>(),
            ["newer", "older"]
        );
    }
}
