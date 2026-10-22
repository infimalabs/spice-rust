//! A Design request asks one unmet question item: the one scoring best
//! against the session's evidence, else the lowest-index one; each item is
//! asked at most once per continuing session, and only the lane's own answer
//! to that item, or its leave, settles the ask.

use wire::rooms::{DesignItemState, RoomQuestion, RoomQuestionItem};

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

/// The question position and text the lane's owed request asks.
fn asked(lanes: &Lanes, config: &SurfaceConfig) -> Option<(u16, String, DesignItemState)> {
    let ask = lanes.request(config, FIRST)?.ask?;
    let position = u16::try_from(ask.question.offset() + 1).expect("fixture position");
    Some((position, ask.text, ask.state))
}

impl Lanes {
    /// Charge question item `question` of `design` with `value`.
    fn charge(
        &self,
        id: u8,
        config: &SurfaceConfig,
        lane: u8,
        design: &wire::TaskHandle,
        question: u16,
        value: i8,
    ) {
        self.contribute_kind(
            id,
            config,
            lane,
            design,
            wire::rooms::RoomContributionKind::charge(value),
            wire::rooms::RoomQuestion::new(question),
        );
    }
}

#[test]
fn an_item_ask_names_the_best_item_and_settles_only_on_the_lanes_answer() {
    let lanes = Lanes::new("design-request-item-ask");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let design = item_design(&lanes, &[UNRELATED[0], RELEVANT]);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let mut opportunity = |stamp: &str| {
        let task = lanes.claimed(stamp, FIRST);
        lanes.complete(&task, &format!("{MATCHING} Completed as {stamp}."));
        lanes.reconcile_profiles(&config, &seated, &mut completions, &profiles);
    };

    opportunity("dVrD245Kc");
    assert_eq!(
        asked(&lanes, &config),
        Some((2, RELEVANT.to_owned(), DesignItemState::Quiet)),
        "the item scoring best against the evidence is asked, not the lowest"
    );
    assert_eq!(
        lanes.noticed().last().and_then(|notice| notice.ask.clone()),
        Some(wire::rooms::DesignAsk {
            question: RoomQuestion::new(2).expect("position"),
            item: RoomQuestionItem::of(RELEVANT),
            state: DesignItemState::Quiet,
        })
    );

    lanes.contribute(50, &config, FIRST, &design);
    lanes.post_listeners(51, &config, &design);
    assert_eq!(
        asked(&lanes, &config).map(|(position, ..)| position),
        Some(2),
        "an open question and another actor's post leave the item unanswered"
    );

    lanes.charge(52, &config, FIRST, &design, 2, 1);
    assert_eq!(asked(&lanes, &config), None);
    opportunity("dVrD245Kp");
    assert_eq!(
        asked(&lanes, &config).map(|(position, ..)| position),
        Some(1),
        "the session's next opportunity asks the item it was not yet asked"
    );

    lanes.charge(53, &config, FIRST, &design, 2, 1);
    assert_eq!(
        asked(&lanes, &config).map(|(position, ..)| position),
        Some(1),
        "a charge against another item leaves the ask owed"
    );
    lanes.leave(54, &config, FIRST, &design);
    assert_eq!(asked(&lanes, &config), None);
    opportunity("dVrD245Kq");
    assert_eq!(lanes.request(&config, FIRST), None);
    assert_eq!(
        lanes.noticed().len(),
        2,
        "each item is asked once per session"
    );
    seated.release_all();
    lanes.finish();
}

#[test]
fn a_whole_statement_match_asks_the_lowest_unheld_item_when_no_item_clears_the_floor() {
    let lanes = Lanes::new("design-request-item-floor");
    let config = surface(&lanes);
    let design = item_design(&lanes, &UNRELATED);
    lanes.charge(50, &config, FIRST, &design, 1, 1);
    align_completion_lane(&lanes);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let task = lanes.claimed("dVrD245Kc", FIRST);
    lanes.complete(&task, MATCHING);
    lanes.reconcile_profiles(&config, &seated, &mut completions, &profiles);

    assert_eq!(
        asked(&lanes, &config),
        Some((2, UNRELATED[1].to_owned(), DesignItemState::Quiet)),
        "the item whose series holds the lane is passed over for the next one"
    );
    seated.release_all();
    lanes.finish();
}

/// The invitation floor the planner reads for `design` now.
fn floor(lanes: &Lanes, config: &SurfaceConfig, design: &wire::TaskHandle) -> f64 {
    let planning =
        crate::resident::room_catch_up::Planning::load(&lanes.bus, config).expect("planning loads");
    let born = stamp(design);
    let entry = planning
        .entries
        .iter()
        .find(|entry| *entry.born() == born)
        .expect("the Design is on the board");
    crate::resident::design_awareness::invitation_floor(planning.snapshot.facts(), entry)
}

#[test]
fn a_steeped_design_invites_at_converges_floor_until_an_item_contests() {
    use wire::rooms::WorkingGroupStance::{Converge, Explore, Passive};

    let lanes = Lanes::new("design-request-steeped-floor");
    lanes.seat(THIRD);
    let config = surface(&lanes);
    let design = item_design(&lanes, &[UNRELATED[0], UNRELATED[1]]);
    lanes.group(30, &config, &design, Passive);
    let expected = crate::resident::design_awareness::match_score_floor;
    assert_eq!(floor(&lanes, &config, &design), expected(Passive));

    lanes.charge(50, &config, SENDER, &design, 1, 1);
    lanes.charge(51, &config, SECOND, &design, 1, 1);
    assert_eq!(
        floor(&lanes, &config, &design),
        expected(Passive),
        "two opinions leave the item unopposed"
    );
    lanes.charge(52, &config, THIRD, &design, 1, 1);
    assert_eq!(
        floor(&lanes, &config, &design),
        expected(Converge),
        "three lanes concur, and a steeped Design asks as Converge does"
    );
    lanes.stance(53, &config, Explore);
    assert_eq!(
        floor(&lanes, &config, &design),
        expected(Converge),
        "Explore cannot lower a steeped Design's floor"
    );

    lanes.charge(54, &config, SENDER, &design, 1, -1);
    assert_eq!(
        floor(&lanes, &config, &design),
        expected(Explore),
        "a contesting charge un-steeps the Design and its stance's floor returns"
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
        lanes.charge(id, &config, lane, &design, 1, 1);
    }
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let task = lanes.claimed("dVrD245Kc", FIRST);
    lanes.complete(&task, MATCHING);
    lanes.reconcile_profiles(&config, &seated, &mut completions, &profiles);
    assert_eq!(
        lanes.request(&config, FIRST),
        None,
        "a Design whose every item concurs has nothing left for an outside lane to weigh"
    );
    assert!(lanes.noticed().is_empty());
    seated.release_all();
    lanes.finish();
}
