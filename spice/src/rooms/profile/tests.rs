use super::*;

fn uuid(last: &str) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last}")
        .parse()
        .expect("fixture uuid")
}

fn declaration(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    identity: &str,
) -> LaneSessionDeclared {
    LaneSessionDeclared {
        lane,
        occupant,
        worktree: "/worktree".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        launch: wire::body::LaneLaunchIdentity::new("model", "effort"),
        epoch: uuid("03"),
        stream: "session".to_owned(),
        source: uuid("04"),
        identity: identity.to_owned(),
        member: 1,
        offset: 2,
        length: 3,
    }
}

fn task(key: u64, title: &str) -> Evidence<EvidenceKey> {
    Evidence::task(
        EvidenceKey::Task {
            task: TaskStamp::for_micros(i64::try_from(key).unwrap()).unwrap(),
            statement: Seq::new(key * 2 + 1).unwrap(),
            completion: Seq::new(key * 2 + 2).unwrap(),
        },
        title,
        None,
        &[],
        "checked",
    )
}

#[test]
fn declarations_share_only_the_exact_continuing_session() {
    let lane = uuid("01");
    let first = uuid("11");
    let resumed = uuid("12");
    let fresh = uuid("13");
    let unknown = uuid("14");
    let mut profiles = SessionProfiles::default();

    let continuing = profiles.declare(&declaration(lane, first, "session-a"));
    assert_eq!(
        profiles.insert(lane, first, task(1, "retained work")),
        Inserted::Stored
    );
    assert_eq!(
        profiles.insert(lane, unknown, task(2, "unknown work")),
        Inserted::Undeclared
    );

    let resumed_key = profiles.declare(&declaration(lane, resumed, "session-a"));
    let fresh_key = profiles.declare(&declaration(lane, fresh, "session-b"));
    assert_eq!(resumed_key, continuing);
    assert_ne!(fresh_key, continuing);
    assert_eq!(
        profiles
            .profile(&resumed_key)
            .unwrap()
            .evidence()
            .evidence()
            .count(),
        1
    );
    assert!(matches!(
        profiles
            .profile(&resumed_key)
            .unwrap()
            .evidence()
            .evidence()
            .next()
            .unwrap()
            .key(),
        EvidenceKey::Task {
            statement,
            completion,
            ..
        } if *statement == Seq::new(3).unwrap() && *completion == Seq::new(4).unwrap()
    ));
    assert_eq!(
        profiles
            .profile(&fresh_key)
            .unwrap()
            .evidence()
            .evidence()
            .count(),
        0
    );
}

#[test]
fn warming_never_makes_incomplete_or_failed_history_look_empty() {
    let lane = uuid("01");
    let occupant = uuid("11");
    let mut profiles = SessionProfiles::default();
    let key = profiles.declare(&declaration(lane, occupant, "session-a"));

    profiles.begin_warming(&key);
    assert_eq!(
        profiles.profile(&key).unwrap().warm_state(),
        &WarmState::Warming {
            cursor: WarmCursor::default()
        }
    );
    let cursor = WarmCursor {
        facts: Seq::new(20),
        ..WarmCursor::default()
    };
    profiles.warmed(&key, cursor, false);
    assert_eq!(
        profiles.profile(&key).unwrap().warm_state(),
        &WarmState::Incomplete { cursor }
    );
    profiles.warm_failed(&key, "source unavailable");
    assert_eq!(
        profiles.profile(&key).unwrap().warm_state(),
        &WarmState::Failed {
            cursor,
            detail: "source unavailable".to_owned()
        }
    );
}

#[test]
fn opportunities_are_cursor_addressed_and_exact_repeats_are_silent() {
    let lane = uuid("01");
    let occupant = uuid("11");
    let mut profiles = SessionProfiles::default();
    let key = profiles.declare(&declaration(lane, occupant, "session-a"));
    let evidence = task(1, "retained work");
    let evidence_key = evidence.key().clone();
    let digest = evidence.digest();

    assert_eq!(
        profiles.insert(lane, occupant, evidence.clone()),
        Inserted::Stored
    );
    assert_eq!(
        profiles.insert(lane, occupant, evidence),
        Inserted::Duplicate
    );
    profiles.begin_warming(&key);
    profiles.warmed(&key, WarmCursor::default(), true);
    profiles.warmed_facts(None, true);
    profiles.design_changed(Seq::new(30).unwrap());

    let first = profiles.opportunities(None, 2);
    assert_eq!(first.rows.len(), 2);
    assert!(first.more);
    assert!(matches!(
        &first.rows[0].kind,
        ProfileOpportunityKind::Declared { occupant: seen } if *seen == occupant
    ));
    assert!(matches!(
        &first.rows[1].kind,
        ProfileOpportunityKind::Stored { key, digest: seen }
            if key == &evidence_key && *seen == digest
    ));
    let rest = profiles.opportunities(first.next, PROFILE_OPPORTUNITIES);
    assert_eq!(rest.rows.len(), 2);
    assert!(!rest.more);
    assert!(matches!(
        rest.rows[0].kind,
        ProfileOpportunityKind::Ready { .. }
    ));
    assert!(matches!(
        rest.rows[1].kind,
        ProfileOpportunityKind::DesignChanged { .. }
    ));
}

#[test]
fn opportunity_cursor_reports_when_bounded_retention_was_crossed() {
    let lane = uuid("01");
    let occupant = uuid("11");
    let mut profiles = SessionProfiles::default();
    profiles.declare(&declaration(lane, occupant, "session-a"));
    for revision in 1..=PROFILE_OPPORTUNITIES + 1 {
        profiles.design_changed(Seq::new(u64::try_from(revision).unwrap()).unwrap());
    }

    let page = profiles.opportunities(Some(1), 1);
    assert!(page.missed);
    assert_eq!(page.rows.len(), 1);
    assert!(page.more);
}
