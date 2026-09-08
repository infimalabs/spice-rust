//! Two-client optimistic concurrency across the complete topology surface.

use super::*;

impl Arranged {
    fn arrange_lane_at(
        &self,
        id: u8,
        topology_revision: u64,
        lane: CanonicalUuid,
        before: CanonicalUuid,
    ) -> Outcome<CommandAnswer> {
        let request = wire::intent::LaneArrange {
            topology_revision,
            lane,
            before,
        };
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
            epoch: self.epoch,
            kind: CommandKind::LaneArrange,
            expected: Vec::new(),
            body: serde_json::to_value(request).expect("lane arrangement encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: self.epoch,
        })
        .expect("operator lane arrangement admits");
        lane_surface::arrange(&self.bus, self.epoch, &self.seats, &command, &request)
    }

    fn configure_at(
        &self,
        id: u8,
        topology_revision: u64,
        lineage_id: CanonicalUuid,
        life: wire::intent::LineageLife,
    ) -> Outcome<CommandAnswer> {
        let request = wire::intent::LineageConfigure {
            topology_revision,
            lineage_id,
            member: None,
            life: Some(life),
        };
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
            epoch: self.epoch,
            kind: CommandKind::LineageConfigure,
            expected: Vec::new(),
            body: serde_json::to_value(&request).expect("lineage configuration encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: self.epoch,
        })
        .expect("operator lineage configuration admits");
        lane_surface::configure(&self.bus, self.epoch, &self.seats, &command, &request)
    }

    fn configure_member_at(
        &self,
        id: u8,
        topology_revision: u64,
        lineage_id: CanonicalUuid,
        member: CanonicalUuid,
    ) -> Outcome<CommandAnswer> {
        let request = wire::intent::LineageConfigure {
            topology_revision,
            lineage_id,
            member: Some(member),
            life: None,
        };
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
            epoch: self.epoch,
            kind: CommandKind::LineageConfigure,
            expected: Vec::new(),
            body: serde_json::to_value(&request).expect("lineage membership encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: self.epoch,
        })
        .expect("operator lineage membership admits");
        lane_surface::configure(&self.bus, self.epoch, &self.seats, &command, &request)
    }

    fn committed(outcome: Outcome<CommandAnswer>) -> Box<wire::intent::LaneAnswer> {
        committed_lane_answer(outcome)
    }

    fn stale(outcome: Outcome<CommandAnswer>, requested: u64, current: u64) {
        let Outcome::Refusal(refusal) = outcome else {
            panic!("stale topology mutation must refuse: {outcome:?}");
        };
        let diagnostic = refusal
            .diagnostic()
            .expect("stale topology mutation carries a predicate diagnostic");
        assert_eq!(diagnostic.reason(), "lane::topology-moved");
        assert_eq!(diagnostic.witness(), Some("topology_revision"));
        assert_eq!(diagnostic.expected(), Some(requested.to_string().as_str()));
        assert_eq!(diagnostic.observed(), Some(current.to_string().as_str()));
    }

    fn committed_arrangement(
        &self,
        id: u8,
        topology_revision: u64,
        lineages: Vec<wire::intent::LineageArrangement>,
    ) -> Box<wire::intent::LaneAnswer> {
        committed_lane_answer(self.arrange_at(id, topology_revision, lineages))
    }
}

/// Two clients may each hold a valid topology, but only the first mutation
/// against their shared revision may commit.
///
/// The sequence deliberately reuses each winning client's old revision for a
/// second client. Every losing body remains independently valid: especially
/// the first pair, where both complete arrangements name every seated lane
/// exactly once. Exact durable-head equality after each loss proves stale
/// refusal happens without mutation.
#[test]
fn two_clients_share_one_revision_authority_across_every_topology_mutation() {
    let fixture = Arranged::seat("topology-revision-concurrency", &[0, 0, 1, 1]);
    let [first_org, second_org] = fixture.organizations;
    let lanes = fixture.lanes.clone();

    let shared = fixture.head().topology_revision;
    let reordered = fixture.committed_arrangement(
        40,
        shared,
        vec![
            standing(second_org, &[lanes[2], lanes[3]]),
            standing(first_org, &[lanes[0], lanes[1]]),
        ],
    );
    assert_eq!(reordered.topology_revision, shared + 1);
    let after_reorder = fixture.head();
    Arranged::stale(
        fixture.arrange_at(41, shared, vec![standing(first_org, &lanes)]),
        shared,
        after_reorder.topology_revision,
    );
    assert_eq!(fixture.head(), after_reorder);

    let joined = fixture.committed_arrangement(
        42,
        after_reorder.topology_revision,
        vec![
            standing(second_org, &[lanes[2], lanes[3], lanes[1]]),
            standing(first_org, &[lanes[0]]),
        ],
    );
    assert_eq!(
        joined.topology_revision,
        after_reorder.topology_revision + 1
    );
    let after_join = fixture.head();
    Arranged::stale(
        fixture.arrange_at(
            43,
            after_reorder.topology_revision,
            vec![
                standing(second_org, &[lanes[2], lanes[3]]),
                standing(first_org, &[lanes[0], lanes[1]]),
            ],
        ),
        after_reorder.topology_revision,
        after_join.topology_revision,
    );
    assert_eq!(fixture.head(), after_join);

    let split = fixture.committed_arrangement(
        44,
        after_join.topology_revision,
        vec![
            standing(second_org, &[lanes[2], lanes[3]]),
            standing(first_org, &[lanes[0], lanes[1]]),
        ],
    );
    assert_eq!(split.topology_revision, after_join.topology_revision + 1);

    let member_shared = fixture.head().topology_revision;
    let arranged_member =
        Arranged::committed(fixture.arrange_lane_at(45, member_shared, lanes[1], lanes[0]));
    assert_eq!(arranged_member.topology_revision, member_shared + 1);
    assert_eq!(
        arranged_member
            .lanes
            .iter()
            .map(|row| row.lane)
            .collect::<Vec<_>>(),
        vec![lanes[2], lanes[3], lanes[1], lanes[0]],
    );
    let after_member = fixture.head();
    Arranged::stale(
        fixture.arrange_lane_at(46, member_shared, lanes[0], lanes[2]),
        member_shared,
        after_member.topology_revision,
    );
    assert_eq!(fixture.head(), after_member);

    let policy_shared = after_member.topology_revision;
    let configured = Arranged::committed(fixture.configure_at(
        47,
        policy_shared,
        first_org,
        wire::intent::LineageLife::Drain,
    ));
    assert_eq!(configured.topology_revision, policy_shared + 1);
    let after_policy = fixture.head();
    Arranged::stale(
        fixture.configure_at(
            48,
            policy_shared,
            second_org,
            wire::intent::LineageLife::Steer,
        ),
        policy_shared,
        after_policy.topology_revision,
    );
    assert_eq!(fixture.head(), after_policy);
    assert_eq!(
        after_policy
            .subscriptions
            .iter()
            .find(|subscription| subscription.lineage_id == second_org)
            .expect("second organization keeps a subscription")
            .policy,
        wire::intent::LineageLife::Drive,
    );

    fixture.retire();
}

#[test]
fn membership_only_configuration_preserves_existing_lifetimes() {
    let fixture = Arranged::seat("membership-configuration-preserves-life", &[0, 0, 1, 1]);
    let [first_org, second_org] = fixture.organizations;
    let lanes = fixture.lanes.clone();
    let configured = Arranged::committed(fixture.configure_at(
        49,
        fixture.head().topology_revision,
        first_org,
        wire::intent::LineageLife::Drain,
    ));
    let moved = Arranged::committed(fixture.configure_member_at(
        50,
        configured.topology_revision,
        first_org,
        lanes[2],
    ));
    assert_eq!(
        moved
            .lanes
            .iter()
            .find(|row| row.lane == lanes[2])
            .expect("moved member remains seated")
            .lineage_id,
        first_org,
    );
    assert_eq!(
        moved
            .subscriptions
            .iter()
            .find(|subscription| subscription.lineage_id == first_org)
            .expect("configured organization keeps a subscription")
            .policy,
        wire::intent::LineageLife::Drain,
        "a membership-only patch cannot restate a stale lifetime",
    );
    assert_eq!(
        moved
            .subscriptions
            .iter()
            .find(|subscription| subscription.lineage_id == second_org)
            .expect("source organization keeps a subscription")
            .policy,
        wire::intent::LineageLife::Drive,
    );

    fixture.retire();
}
