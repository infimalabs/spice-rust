//! Stable direction identities shared by quoted Chat cards and global history.

use std::collections::BTreeSet;

use wire::CanonicalUuid;

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
    use std::collections::BTreeSet;

    use super::direction_identity;

    fn id(suffix: u8) -> wire::CanonicalUuid {
        format!("00000000-0000-4000-8000-{suffix:012}")
            .parse()
            .expect("fixture identity is canonical")
    }

    #[test]
    fn identity_uses_fanout_and_only_searches_team_members() {
        let authored = "2030-01-01T00:00:00.000Z".parse::<wire::Instant>().unwrap();
        let retired = "2030-01-01T00:01:00.000Z".parse::<wire::Instant>().unwrap();
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
                        fanout: Some(id(9)),
                        text: "direction".to_owned(),
                        from: None,
                        preview: Some("direction".to_owned()),
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
                        fanout: Some(id(7)),
                        from: None,
                        text: "direction".to_owned(),
                        preview: Some("direction".to_owned()),
                        attachments: Vec::new(),
                        authored,
                        disposition: wire::Disposition::Acked,
                        position: wire::Seq(std::num::NonZeroU64::new(1).unwrap()),
                        retired,
                    }],
                },
            ],
        };

        assert_eq!(
            direction_identity(&BTreeSet::from([id(1)]), &inbox, "1kLive"),
            Some(id(9))
        );
        assert_eq!(
            direction_identity(&BTreeSet::from([id(1)]), &inbox, "1kDone"),
            None
        );
        assert_eq!(
            direction_identity(&BTreeSet::from([id(2)]), &inbox, "1kDone"),
            Some(id(7))
        );
    }
}
