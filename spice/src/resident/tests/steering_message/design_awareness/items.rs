//! A Design request asks every question item pending for the lane, the one
//! scoring best against the session's evidence first and the rest in
//! statement order; the lane's own answer to any of them, or its leave,
//! settles the request for the session, and a later session is owed only the
//! items the lane has not weighed. When no whole Design clears its floor, one
//! quiet item clearing the item floor admits a request asking only that item.

use wire::rooms::{DesignItemState, RoomQuestionItem};

use super::*;

/// The Design's problem sentence, sharing nothing with the lane's work.
const OFF_TOPIC: &str = "Zebras hum lullabies near glaciers.";
/// Items sharing nothing with the lane's work.
const UNRELATED: [&str; 3] = [
    "Should zebras hum louder near glaciers?",
    "Do glaciers keep zebras awake at night?",
    "Can lullabies calm restless glaciers?",
];
/// An item restating the lane's completed work.
const RELEVANT: &str = "Do retained publications page through bounded cursors?";

/// A Design whose whole statement matches the lane's work while its problem
/// sentence does not.
fn item_design(lanes: &Lanes, questions: &[&str]) -> wire::TaskHandle {
    lanes.design_with_questions(
        "dVrD245Kd",
        TASK_TITLE,
        &format!("{OFF_TOPIC} {PITCH}"),
        questions,
    )
}

/// The question positions and states the lane's owed request asks, in order.
fn asked(lanes: &Lanes, config: &SurfaceConfig) -> Option<Vec<(u16, DesignItemState)>> {
    let request = lanes.request(config, FIRST)?;
    Some(
        request
            .asks
            .iter()
            .map(|ask| {
                let position = u16::try_from(ask.question.offset() + 1).expect("fixture position");
                (position, ask.state)
            })
            .collect(),
    )
}

impl Lanes {
    /// Charge question item `question` of `design` with `value`.
    fn charge(&self, id: u8, lane: u8, design: &wire::TaskHandle, question: u16, value: i8) {
        self.contribute_kind(
            id,
            lane,
            design,
            wire::rooms::RoomContributionKind::charge(value),
            wire::rooms::RoomQuestion::new(question),
        );
    }
}

#[test]
fn one_request_asks_every_pending_item_and_one_answer_settles_the_session() {
    use DesignItemState::Quiet;

    let lanes = Lanes::new("design-request-item-asks");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let design = item_design(&lanes, &[UNRELATED[0], RELEVANT, UNRELATED[1]]);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let opportunity = |stamp: &str, seated: &Seats, completions: &mut _| {
        let task = lanes.claimed(stamp, FIRST);
        lanes.complete(&task, &format!("{MATCHING} Completed as {stamp}."));
        lanes.reconcile_profiles(seated, completions, &profiles);
    };

    opportunity("dVrD245Kc", &seated, &mut completions);
    assert_eq!(
        asked(&lanes, &config),
        Some(vec![(2, Quiet), (1, Quiet), (3, Quiet)]),
        "the best-scoring item leads and the rest keep statement order"
    );
    let notices = lanes.noticed();
    assert_eq!(notices.len(), 1, "one notice carries every pending item");
    assert_eq!(
        notices[0]
            .asks
            .iter()
            .map(|ask| ask.item.clone())
            .collect::<Vec<_>>(),
        [RELEVANT, UNRELATED[0], UNRELATED[1]].map(RoomQuestionItem::of)
    );
    let text = spice::rooms::awareness::request_text(
        &lanes.request(&config, FIRST).expect("the request is owed"),
    );
    for question in [RELEVANT, UNRELATED[0], UNRELATED[1]] {
        assert_eq!(text.matches(question).count(), 1, "{text}");
    }
    assert_eq!(text.matches("spice room contribute").count(), 1, "{text}");

    lanes.contribute(50, FIRST, &design);
    lanes.post_listeners(51, &config, &design);
    assert!(
        asked(&lanes, &config).is_some(),
        "an open question and another actor's post leave the request owed"
    );

    lanes.charge(52, FIRST, &design, 2, 1);
    assert_eq!(
        asked(&lanes, &config),
        None,
        "an answer to any item settles it"
    );
    opportunity("dVrD245Kp", &seated, &mut completions);
    assert_eq!(lanes.request(&config, FIRST), None);
    assert_eq!(
        lanes.noticed().len(),
        1,
        "a settled request is not reissued for its other items in the session"
    );

    seated.release_all();
    let successor = test_uuid(FIRST.saturating_add(40));
    lanes.vacate(FIRST);
    lanes.renew(FIRST, successor);
    lanes.declare_identity(FIRST, test_uuid(FIRST.saturating_add(41)).to_string());
    seated = seats(&[(FIRST, successor)]);
    opportunity("dVrD245Kq", &seated, &mut completions);
    assert_eq!(
        asked(&lanes, &config),
        Some(vec![(1, Quiet), (3, Quiet)]),
        "a later session is owed only the items the lane has not weighed"
    );
    seated.release_all();
    lanes.finish();
}

#[test]
fn a_whole_statement_match_asks_the_lowest_unheld_item_when_no_item_clears_the_floor() {
    let lanes = Lanes::new("design-request-item-floor");
    let config = surface(&lanes);
    let design = item_design(&lanes, &UNRELATED);
    lanes.charge(50, FIRST, &design, 1, 1);
    align_completion_lane(&lanes);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let task = lanes.claimed("dVrD245Kc", FIRST);
    lanes.complete(&task, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);

    assert_eq!(
        asked(&lanes, &config),
        Some(vec![
            (2, DesignItemState::Quiet),
            (3, DesignItemState::Quiet)
        ]),
        "the item whose series holds the lane is passed over"
    );
    seated.release_all();
    lanes.finish();
}

/// The invitation floor the planner reads for `design` now.
fn floor(lanes: &Lanes, design: &wire::TaskHandle) -> f64 {
    let planning =
        crate::resident::room_catch_up::Planning::load(&lanes.bus).expect("planning loads");
    let born = stamp(design);
    let entry = planning
        .entries
        .iter()
        .find(|entry| *entry.born() == born)
        .expect("the Design is on the board");
    crate::resident::design_awareness::invitation_floor(planning.snapshot.facts(), entry)
}

#[test]
fn a_steeped_design_invites_narrow_until_an_item_contests() {
    use wire::rooms::DesignReach::{Narrow, Wide};

    let lanes = Lanes::new("design-request-steeped-floor");
    lanes.seat(THIRD);
    let config = surface(&lanes);
    let design = item_design(&lanes, &[UNRELATED[0], UNRELATED[1]]);
    lanes.group(30, &config, &design);
    let expected = crate::resident::design_awareness::match_score_floor;
    assert_eq!(floor(&lanes, &design), expected(Wide));

    lanes.charge(50, SENDER, &design, 1, 1);
    lanes.charge(51, SECOND, &design, 1, 1);
    lanes.charge(52, THIRD, &design, 1, 1);
    assert_eq!(
        floor(&lanes, &design),
        expected(Wide),
        "three lanes concur on one item, but a quiet item keeps the reach wide"
    );

    lanes.charge(54, SECOND, &design, 2, 1);
    assert_eq!(
        floor(&lanes, &design),
        expected(Narrow),
        "every item heard and one concurring steeps the Design, which narrows its reach"
    );

    lanes.charge(57, SENDER, &design, 1, -1);
    assert_eq!(
        floor(&lanes, &design),
        expected(Wide),
        "a contesting charge un-steeps the Design and widens its reach again"
    );
    lanes.finish();
}

#[test]
fn every_item_concurring_owes_no_whole_design_ask() {
    let lanes = Lanes::new("design-request-all-concurred");
    lanes.seat(THIRD);
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let design = item_design(&lanes, &[UNRELATED[0]]);
    for (id, lane) in [(50, SENDER), (51, SECOND), (52, THIRD)] {
        lanes.charge(id, lane, &design, 1, 1);
    }
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let task = lanes.claimed("dVrD245Kc", FIRST);
    lanes.complete(&task, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert_eq!(
        lanes.request(&config, FIRST),
        None,
        "a Design whose every item concurs has nothing left for an outside lane to weigh"
    );
    assert!(lanes.noticed().is_empty());
    seated.release_all();
    lanes.finish();
}

/// A Design whose long statement, about something else, dilutes any one item
/// below the wide floor. Each `stamp` gets its own wording so no two Designs
/// share the vocabulary that dilutes them.
fn diluted_design(lanes: &Lanes, stamp: &str, questions: &[&str]) -> wire::TaskHandle {
    const ONSETS: [&str; 20] = [
        "zor", "vex", "glim", "quar", "tund", "brak", "flin", "morv", "sild", "thra", "welk",
        "yarm", "drel", "kosp", "nurf", "plok", "gusk", "hond", "jilt", "lumb",
    ];
    let prefix = stamp.chars().last().expect("fixture stamp");
    let mut statement = String::new();
    for first in ONSETS {
        for second in ONSETS {
            statement.push_str(&format!("{prefix}{first}{second} "));
        }
    }
    lanes.design_with_questions(stamp, "Hum zebras to sleep", &statement, questions)
}

/// Complete one task under `FIRST` matching the lane's retained work and
/// reconcile the session profiles.
fn complete_matching(
    lanes: &Lanes,
    stamp: &str,
    seated: &Seats,
    completions: &mut crate::resident::design_awareness::CompletionCursor,
    profiles: &std::sync::Mutex<crate::resident::message_surface::MessageState>,
) {
    let task = lanes.claimed(stamp, FIRST);
    lanes.complete(&task, &format!("{MATCHING} Completed as {stamp}."));
    lanes.reconcile_profiles(seated, completions, profiles);
}

#[test]
fn a_quiet_item_admits_a_request_asking_only_it_once_per_session() {
    let lanes = Lanes::new("design-request-item-admission");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let design = diluted_design(&lanes, "dVrD245Kd", &[UNRELATED[0], RELEVANT, UNRELATED[1]]);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());

    complete_matching(&lanes, "dVrD245Kc", &seated, &mut completions, &profiles);
    let notices = lanes.noticed();
    assert_eq!(notices.len(), 1, "one quiet item admits one request");
    let DesignAdmission::Scored(matching) = &notices[0].admission else {
        panic!("item admission is scored: {:?}", notices[0].admission);
    };
    assert_eq!(matching.reach(), DesignReach::Item);
    assert_eq!(matching.floor(), board::similarity::ITEM_MATCH_FLOOR);
    assert!(matching.score() >= matching.floor(), "{}", matching.score());
    assert_eq!(
        notices[0]
            .asks
            .iter()
            .map(|ask| (ask.item.clone(), ask.state))
            .collect::<Vec<_>>(),
        [(RoomQuestionItem::of(RELEVANT), DesignItemState::Quiet)],
        "the request asks only the item that matched"
    );
    let text = spice::rooms::awareness::request_text(
        &lanes
            .request(&config, FIRST)
            .expect("the item request is owed"),
    );
    assert_eq!(text.matches(RELEVANT).count(), 1, "{text}");
    for unasked in [UNRELATED[0], UNRELATED[1]] {
        assert!(!text.contains(unasked), "{text}");
    }

    lanes.leave(50, FIRST, &design);
    assert_eq!(lanes.request(&config, FIRST), None);
    complete_matching(&lanes, "dVrD245Kp", &seated, &mut completions, &profiles);
    assert_eq!(
        lanes.noticed().len(),
        1,
        "an item asked and answered in the session is not admitted again"
    );
    seated.release_all();
    lanes.finish();
}

/// Complete one task matching the lane's work and report whether any request
/// was recorded.
fn admits_any(lanes: &Lanes) -> bool {
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    complete_matching(lanes, "dVrD245Kc", &seated, &mut completions, &profiles);
    seated.release_all();
    !lanes.noticed().is_empty()
}

#[test]
fn a_quiet_item_below_the_item_floor_admits_nothing() {
    let lanes = Lanes::new("design-request-item-below");
    align_completion_lane(&lanes);
    diluted_design(&lanes, "dVrD245Kd", &UNRELATED);
    assert!(!admits_any(&lanes));
    lanes.finish();
}

#[test]
fn an_unopposed_contested_or_concurred_item_never_admits_by_itself() {
    let lanes = Lanes::new("design-request-item-heard");
    lanes.seat(THIRD);
    align_completion_lane(&lanes);
    let unopposed = diluted_design(&lanes, "dVrD245Km", &[RELEVANT]);
    let contested = diluted_design(&lanes, "dVrD245Kn", &[RELEVANT]);
    let concurred = diluted_design(&lanes, "dVrD245Kg", &[RELEVANT]);
    lanes.charge(50, SECOND, &unopposed, 1, 1);
    lanes.charge(51, SECOND, &contested, 1, 1);
    lanes.charge(52, THIRD, &contested, 1, -1);
    for (id, lane) in [(53, SENDER), (54, SECOND), (55, THIRD)] {
        lanes.charge(id, lane, &concurred, 1, 1);
    }
    assert!(!admits_any(&lanes));
    lanes.finish();
}

#[test]
fn a_design_clearing_its_own_floor_is_asked_instead_of_any_item() {
    let lanes = Lanes::new("design-request-item-yields");
    align_completion_lane(&lanes);
    let whole = item_design(&lanes, &[RELEVANT]);
    diluted_design(&lanes, "dVrD245Km", &[UNRELATED[0], RELEVANT]);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());

    complete_matching(&lanes, "dVrD245Kc", &seated, &mut completions, &profiles);
    let notices = lanes.noticed();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].design, stamp(&whole));
    let DesignAdmission::Scored(matching) = &notices[0].admission else {
        panic!(
            "Design-level admission is scored: {:?}",
            notices[0].admission
        );
    };
    assert_eq!(matching.reach(), DesignReach::Wide);
    seated.release_all();
    lanes.finish();
}
