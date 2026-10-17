use super::super::evidence::{CONVERSATION_CARDS, Conversation, TASK_CARDS};
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

fn crossed(offset: u64) -> TranscriptCursor {
    TranscriptCursor {
        source: uuid("99"),
        member: 1,
        offset,
    }
}

fn conversation(index: usize) -> Evidence<EvidenceKey> {
    let letter = |value: usize| char::from(b'a' + u8::try_from(value % 26).unwrap());
    let prose = (0..12)
        .map(|term| {
            format!(
                "note{}{}{}",
                letter(index / 26),
                letter(index),
                letter(term)
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    Evidence::conversation(
        EvidenceKey::Conversation {
            source: uuid("99"),
            member: 1,
            offset: u64::try_from(index).unwrap(),
            length: 1,
        },
        &Conversation::read(&prose).expect("twelve distinct terms admit"),
    )
}

fn ready_count(profiles: &SessionProfiles) -> usize {
    profiles
        .opportunities(None, PROFILE_OPPORTUNITIES)
        .rows
        .iter()
        .filter(|row| matches!(row.kind, ProfileOpportunityKind::Ready { .. }))
        .count()
}

#[test]
fn warming_resumes_from_recorded_cursors_until_both_histories_begin() {
    let mut profiles = SessionProfiles::default();
    let key = profiles.declare(&declaration(uuid("01"), uuid("11"), "session-a"));
    profiles.begin_warming(&key);
    assert_eq!(profiles.owed_facts(), Some(None));
    assert_eq!(profiles.owed_transcript(), Some((&key, None)));

    profiles.warmed_facts(Seq::new(20), false);
    profiles.warmed_transcript(&key, crossed(500), false);
    assert_eq!(profiles.owed_facts(), Some(Seq::new(20)));
    assert_eq!(profiles.owed_transcript(), Some((&key, Some(crossed(500)))));
    assert!(matches!(
        profiles.profile(&key).unwrap().warm_state(),
        WarmState::Incomplete { .. }
    ));

    profiles.warmed_transcript(&key, crossed(0), true);
    assert_eq!(profiles.owed_transcript(), None);
    assert_eq!(
        ready_count(&profiles),
        0,
        "completion history is still owed"
    );
    profiles.warmed_facts(None, true);
    assert!(profiles.profile(&key).unwrap().warm_state().is_ready());
    assert_eq!(profiles.owed_facts(), None);
    profiles.warmed_facts(None, true);
    profiles.settle_full();
    assert_eq!(ready_count(&profiles), 1, "readiness is announced once");
}

#[test]
fn full_classes_complete_warming_without_older_history_displacing_them() {
    let lane = uuid("01");
    let occupant = uuid("11");
    let mut profiles = SessionProfiles::default();
    let key = profiles.declare(&declaration(lane, occupant, "session-a"));
    profiles.begin_warming(&key);
    for index in 0..TASK_CARDS {
        let index = u64::try_from(index).unwrap();
        assert_eq!(
            profiles.insert(lane, occupant, task(index + 100, &format!("live {index}"))),
            Inserted::Stored
        );
    }
    assert_eq!(
        profiles.retain_older(&key, task(1, "older work")),
        Inserted::Full
    );
    profiles.settle_full();
    assert_eq!(
        profiles.owed_facts(),
        None,
        "a full task class owes nothing"
    );
    assert!(profiles.owed_transcript().is_some());

    for index in 0..CONVERSATION_CARDS {
        assert_eq!(
            profiles.retain_older(&key, conversation(index)),
            Inserted::Stored
        );
    }
    assert_eq!(
        profiles.retain_older(&key, conversation(CONVERSATION_CARDS)),
        Inserted::Full
    );
    profiles.settle_full();
    assert!(profiles.profile(&key).unwrap().warm_state().is_ready());
    assert_eq!(ready_count(&profiles), 1);

    let newest = profiles
        .profile(&key)
        .unwrap()
        .evidence()
        .evidence()
        .map(|card| card.key().clone())
        .collect::<Vec<_>>();
    assert_eq!(newest.first(), Some(task(131, "").key()));
    assert_eq!(
        newest.last(),
        Some(conversation(CONVERSATION_CARDS - 1).key()),
        "the walk's oldest find is the oldest card"
    );
}

#[test]
fn failed_warming_keeps_its_cursor_and_cards_distinct_from_ready() {
    let lane = uuid("01");
    let occupant = uuid("11");
    let mut profiles = SessionProfiles::default();
    let key = profiles.declare(&declaration(lane, occupant, "session-a"));
    profiles.begin_warming(&key);
    profiles.retain_older(&key, task(1, "found before failing"));
    profiles.warmed_facts(Seq::new(20), false);
    profiles.facts_failed("history unreadable");

    let profile = profiles.profile(&key).unwrap();
    assert!(matches!(
        profile.warm_state(),
        WarmState::Failed { cursor, .. } if cursor.facts == Seq::new(20)
    ));
    assert_eq!(profile.evidence().evidence().count(), 1);
    assert_eq!(profiles.owed_facts(), None);
    assert_eq!(profiles.owed_transcript(), None);
    profiles.warmed_facts(None, true);
    profiles.warmed_transcript(&key, crossed(0), true);
    assert!(!profiles.profile(&key).unwrap().warm_state().is_ready());
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
    profiles.warmed_transcript(&key, crossed(0), true);
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
