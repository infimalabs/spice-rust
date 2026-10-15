//! The crossing that restates each live team's formation instant under the one
//! teamed predicate.

use super::*;

const A: &str = "018f82d4-6f59-7d06-92e3-bce913116321";
const B: &str = "018f82d4-6f59-7d06-92e3-bce913116322";
const C: &str = "018f82d4-6f59-7d06-92e3-bce913116323";
/// The team Create new team gave A, which B later joined.
const SPLIT: &str = "018f82d4-6f59-7d06-92e3-bce913116331";
/// The team Create new team gave C alone.
const SOLO: &str = "018f82d4-6f59-7d06-92e3-bce913116332";

fn instant(text: &str) -> Instant {
    text.parse().expect("fixture instant")
}

/// One residency head over `lanes`, one subscription per team in team order,
/// each team formed at the instant `formed` names.
fn head(lanes: &[(&str, &str)], formed: &[(&str, Instant)]) -> ResidencyHead {
    let teams = lanes
        .iter()
        .map(|(_, team)| uuid(team))
        .collect::<BTreeSet<_>>();
    ResidencyHead {
        lanes: lanes
            .iter()
            .map(|(lane, team)| residency_lane(lane, team, "drive"))
            .collect(),
        subscriptions: teams
            .into_iter()
            .map(|team| wire::TeamSubscription {
                team,
                formed_at: formed
                    .iter()
                    .find(|(formed, _)| uuid(formed) == team)
                    .map(|(_, at)| *at),
                policy: wire::intent::TeamLife::Drive,
                fast_mode: false,
            })
            .collect(),
        ..residency_head()
    }
}

fn commit<F: wire::Body>(journal: &mut Journal, at: Instant, facts: Vec<F>, head: ResidencyHead) {
    let mut write = crate::JournalWrite::new(
        Actor::Spice,
        "resident:018f82d4-6f59-7d06-92e3-bce913116399"
            .parse()
            .expect("fixture origin"),
        at,
        wire::Producer::ResidentRecognition,
    );
    let mut latest = None;
    for fact in facts {
        latest = Some(
            write
                .fact(fact, crate::FactCause::Root)
                .expect("fixture topology fact builds"),
        );
    }
    write.residency_head(latest.expect("a topology commit names a fact"), move |_| {
        head
    });
    journal.apply(write).expect("fixture topology commits");
}

fn seated(lane: &str) -> wire::body::LaneSeated {
    let row = residency_lane(lane, lane, "drive");
    wire::body::LaneSeated {
        lane: row.lane,
        team: row.team,
        capability: uuid("018f82d4-6f59-7d06-92e3-bce913116390"),
        name: row.name,
        worktree: row.worktree,
        life: row.life,
        vendor: row.vendor,
        model: row.model,
        effort: row.effort,
    }
}

fn arranged(lane: &str, team: &str) -> wire::body::LaneArranged {
    wire::body::LaneArranged {
        lane: uuid(lane),
        from_team: uuid(lane),
        team: uuid(team),
        before: uuid(lane),
        sources: Vec::new(),
    }
}

fn stored_head(journal: &Journal) -> ResidencyHead {
    let body: Vec<u8> = journal
        .connection
        .query_row("SELECT body FROM residency_head", [], |row| row.get(0))
        .expect("residency reads");
    wire::decode(&body).expect("residency decodes")
}

fn plant(journal: &Journal, head: &ResidencyHead) {
    journal
        .connection
        .execute(
            "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
            params![wire::encode(head).expect("planted head encodes")],
        )
        .expect("plant a retained head");
}

#[test]
fn retained_heads_cross_to_each_teams_latest_teamed_instant_from_topology_facts() {
    let seats = instant("2031-01-01T00:00:01.000Z");
    let split = instant("2031-01-01T00:00:02.000Z");
    let joined = instant("2031-01-01T00:00:03.000Z");
    let solo = instant("2031-01-01T00:00:04.000Z");
    for predecessor in [56, 57, 58, 59] {
        let scratch = Scratch::new();
        let path = scratch.path().join("authority.sqlite3");
        let mut journal = authority_file_fixture(&path);
        commit(
            &mut journal,
            seats,
            vec![seated(A), seated(B), seated(C)],
            head(&[(A, A), (B, B), (C, C)], &[]),
        );
        commit(
            &mut journal,
            split,
            vec![arranged(A, SPLIT)],
            head(&[(A, SPLIT), (B, B), (C, C)], &[(SPLIT, split)]),
        );
        commit(
            &mut journal,
            joined,
            vec![arranged(B, SPLIT)],
            head(&[(A, SPLIT), (B, SPLIT), (C, C)], &[(SPLIT, split)]),
        );
        commit(
            &mut journal,
            solo,
            vec![arranged(C, SOLO)],
            head(
                &[(A, SPLIT), (B, SPLIT), (C, SOLO)],
                &[(SPLIT, split), (SOLO, solo)],
            ),
        );
        let exact = stored_head(&journal);
        // Before this crossing, formation counted members: the split team was
        // stamped when B joined and the lone split team carried no instant.
        plant(
            &journal,
            &head(&[(A, SPLIT), (B, SPLIT), (C, SOLO)], &[(SPLIT, joined)]),
        );
        restore_predecessor(&journal, predecessor);
        drop(journal);

        let journal = Journal::open(&path).expect("a retained head crosses forward");
        assert_eq!(
            journal.migration().map(|migration| migration.from()),
            Some(predecessor)
        );
        assert_eq!(
            stored_head(&journal),
            exact,
            "schema {predecessor}: each team carries its latest unteamed-to-teamed instant"
        );
        journal.verify().expect("crossed authority verifies");
        drop(journal);

        let reopened = Journal::open(&path).expect("crossed authority reopens");
        assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
        assert_eq!(stored_head(&reopened), exact, "reopening restates nothing");
    }
}

#[test]
fn a_team_without_formation_history_refuses_the_crossing_unchanged() {
    let scratch = Scratch::new();
    let path = scratch.path().join("authority.sqlite3");
    let mut journal = authority_file_fixture(&path);
    commit(
        &mut journal,
        instant("2031-01-01T00:00:01.000Z"),
        vec![seated(C)],
        head(&[(C, C)], &[]),
    );
    plant(&journal, &head(&[(C, SOLO)], &[]));
    restore_predecessor(&journal, 59);
    drop(journal);
    let before = fs::read(&path).expect("authority bytes read");

    let error = Journal::open(&path).expect_err("an unproved formation refuses");
    let OpenError::Migration(MigrationError::RetainedRow { release, .. }) = &error else {
        panic!("expected a retained-row refusal, got {error}");
    };
    assert_eq!(*release, "schema-60");
    assert!(error.to_string().contains(SOLO), "{error}");
    assert_eq!(
        fs::read(&path).expect("authority bytes reread"),
        before,
        "the refusal changes nothing"
    );
}
