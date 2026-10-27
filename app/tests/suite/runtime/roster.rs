//! Teams and working groups share one roster: the same landings, the same
//! full-group refusal, the same drag reorder and the same composer menu.

use super::*;
use app::roster::{Change, Destination, Lift, Side, Target, teams, working_groups};
use wire::CanonicalUuid;
use wire::rooms::{RoomAuthorityStamp, WorkingGroup, WorkingGroupPartition};

const GROUP_A: &str = "dVsYdDj7f";
const GROUP_B: &str = "dVsYdDj7h";

fn group(text: &str) -> RoomAuthorityStamp {
    text.parse().expect("fixture group authority")
}

fn lane(suffix: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131164{suffix:02x}")
        .parse()
        .expect("fixture lane")
}

fn design(label: &str) -> wire::intent::BoardTask {
    let mut task = board_task(
        label,
        label,
        label,
        "PUBLIC",
        wire::TaskFiling::Filed,
        None,
        wire::EnvironmentRequirements::default(),
    );
    task.statement.body.intent = Some(wire::TaskIntent::Design);
    task.workflow.body.flow = vec!["design".parse().expect("fixture phase")];
    task.workflow.body.phase = "design".parse().expect("fixture phase");
    task
}

fn partition(groups: &[(&str, &[&str])]) -> WorkingGroupPartition {
    WorkingGroupPartition {
        revision: 7,
        groups: groups
            .iter()
            .map(|(group_id, designs)| WorkingGroup {
                group: group(group_id),
                anchors: designs.iter().map(|label| fixture_stamp(label)).collect(),
                stance: wire::rooms::WorkingGroupStance::Running,
            })
            .collect(),
        fast: Vec::new(),
    }
}

fn moving<G, M>(member: M, to: Destination<G>) -> Option<Change<G, M>> {
    Some(Change::Move { member, to })
}

#[test]
fn the_ungrouped_list_holds_only_members_no_formed_group_holds() {
    let plain = board_task(
        "PUBLIC-plain",
        "plain",
        "plain",
        "PUBLIC",
        wire::TaskFiling::Filed,
        None,
        wire::EnvironmentRequirements::default(),
    );
    let mut intake = design("D-intake");
    intake.workflow.body.flow = vec![
        "plan".parse().expect("fixture phase"),
        "design".parse().expect("fixture phase"),
    ];
    intake.workflow.body.phase = "plan".parse().expect("fixture phase");
    let tasks = [
        design("D-one"),
        plain,
        intake,
        design("D-two"),
        design("D-three"),
    ];
    let roster = working_groups(&partition(&[(GROUP_A, &["D-two"])]), &tasks);
    assert_eq!(
        roster.ungrouped(),
        [fixture_stamp("D-one"), fixture_stamp("D-three")],
        "Designs lists every ungrouped Design in board order and nothing else",
    );

    let roster = teams(&[
        (lane(0xa0), vec![lane(1), lane(2)]),
        (lane(3), vec![lane(3)]),
        (lane(0xa1), vec![lane(4)]),
    ]);
    assert_eq!(
        roster.ungrouped(),
        [lane(3)],
        "Unassigned Agents lists only lanes seated alone under their own identity",
    );
    assert_eq!(
        roster.groups().map(|(team, _)| *team).collect::<Vec<_>>(),
        [lane(0xa0), lane(0xa1)],
        "a one-agent team under another identity is still a team",
    );
}

#[test]
fn a_full_group_accepts_no_member_from_elsewhere_but_still_reorders() {
    let full = ["F-1", "F-2", "F-3", "F-4", "F-5", "F-6"];
    let tasks = full
        .iter()
        .chain(&["O-1", "loose"])
        .map(|label| design(label))
        .collect::<Vec<_>>();
    let roster = working_groups(&partition(&[(GROUP_A, &full), (GROUP_B, &["O-1"])]), &tasks);
    let (full_group, open_group) = (group(GROUP_A), group(GROUP_B));
    let loose = Lift::Member(fixture_stamp("loose"));

    assert!(!roster.accepts(&full_group));
    assert!(roster.accepts(&open_group));
    assert_eq!(
        roster.landing(&loose, &Target::Group(full_group.clone(), Side::Before)),
        None
    );
    assert_eq!(
        roster.landing(&loose, &Target::Member(fixture_stamp("F-3"), Side::After)),
        None,
        "a tile in a full group refuses a join as its card does",
    );
    assert_eq!(
        roster.landing(&loose, &Target::Group(open_group.clone(), Side::After)),
        moving(fixture_stamp("loose"), Destination::Group(open_group)),
    );
    assert!(matches!(
        roster.landing(
            &Lift::Member(fixture_stamp("F-6")),
            &Target::Member(fixture_stamp("F-1"), Side::Before),
        ),
        Some(Change::Arrange(_))
    ));
}

#[test]
fn dragging_reorders_members_and_groups_and_states_nothing_unchanged() {
    let tasks = ["A-1", "A-2", "B-1"].map(design);
    let roster = working_groups(
        &partition(&[(GROUP_A, &["A-1", "A-2"]), (GROUP_B, &["B-1"])]),
        &tasks,
    );
    let (a, b) = (group(GROUP_A), group(GROUP_B));
    assert_eq!(
        roster.landing(
            &Lift::Member(fixture_stamp("A-2")),
            &Target::Member(fixture_stamp("A-1"), Side::Before),
        ),
        Some(Change::Arrange(vec![
            (a.clone(), vec![fixture_stamp("A-2"), fixture_stamp("A-1")]),
            (b.clone(), vec![fixture_stamp("B-1")]),
        ])),
    );
    assert_eq!(
        roster.landing(
            &Lift::Group(b.clone()),
            &Target::Group(a.clone(), Side::Before)
        ),
        Some(Change::Arrange(vec![
            (b.clone(), vec![fixture_stamp("B-1")]),
            (a.clone(), vec![fixture_stamp("A-1"), fixture_stamp("A-2")]),
        ])),
    );
    assert_eq!(
        roster.landing(
            &Lift::Group(b.clone()),
            &Target::Group(a.clone(), Side::After)
        ),
        None,
        "a group released where it already stands states nothing",
    );
    assert_eq!(
        roster.landing(
            &Lift::Member(fixture_stamp("A-1")),
            &Target::Member(fixture_stamp("A-2"), Side::Before),
        ),
        None,
    );
    assert_eq!(
        roster.landing(
            &Lift::Member(fixture_stamp("B-1")),
            &Target::Member(fixture_stamp("A-1"), Side::After),
        ),
        moving(fixture_stamp("B-1"), Destination::Group(a.clone())),
        "a tile in another group is that group's target",
    );
    assert_eq!(roster.landing(&Lift::Group(a), &Target::New), None);

    let topology = [
        (lane(0xa0), vec![lane(1), lane(2)]),
        (lane(3), vec![lane(3)]),
        (lane(0xa1), vec![lane(4), lane(5)]),
    ];
    assert_eq!(
        teams(&topology).landing(
            &Lift::Group(lane(0xa1)),
            &Target::Group(lane(0xa0), Side::Before)
        ),
        Some(Change::Arrange(vec![
            (lane(0xa1), vec![lane(4), lane(5)]),
            (lane(0xa0), vec![lane(1), lane(2)]),
            (lane(3), vec![lane(3)]),
        ])),
        "a team arrangement restates every row, unassigned ones included",
    );
}

#[test]
fn new_and_ungrouped_targets_state_only_real_moves() {
    let topology = [
        (lane(0xa0), vec![lane(1), lane(2)]),
        (lane(0xa1), vec![lane(4)]),
        (lane(3), vec![lane(3)]),
    ];
    let roster = teams(&topology);
    let lift = |member| Lift::Member(lane(member));
    assert_eq!(
        roster.landing(&lift(3), &Target::New),
        moving(lane(3), Destination::New)
    );
    assert_eq!(
        roster.landing(&lift(1), &Target::New),
        moving(lane(1), Destination::New)
    );
    assert_eq!(
        roster.landing(&lift(4), &Target::New),
        None,
        "a group's only member already stands alone",
    );
    assert_eq!(roster.landing(&lift(3), &Target::Ungrouped), None);
    assert_eq!(
        roster.landing(&lift(4), &Target::Ungrouped),
        moving(lane(4), Destination::Ungrouped),
        "the last member's departure is a move; the authority ends the group",
    );
    assert_eq!(
        roster.landing(&lift(1), &Target::Group(lane(0xa0), Side::After)),
        None
    );
}

#[test]
fn composer_menus_offer_new_group_and_leave_for_both_kinds() {
    let roster = teams(&[
        (lane(0xa0), vec![lane(1), lane(2)]),
        (lane(0xa1), vec![lane(4)]),
        (lane(3), vec![lane(3)]),
    ]);
    let menu = roster.menu(&lane(2));
    assert_eq!(menu.new_group, moving(lane(2), Destination::New));
    assert_eq!(menu.leave, moving(lane(2), Destination::Ungrouped));
    let menu = roster.menu(&lane(4));
    assert_eq!(
        menu.new_group, None,
        "a sole member already has its own team"
    );
    assert_eq!(menu.leave, moving(lane(4), Destination::Ungrouped));
    let menu = roster.menu(&lane(3));
    assert_eq!((menu.new_group, menu.leave), (None, None));

    let tasks = ["A-1", "A-2", "B-1", "loose"].map(design);
    let roster = working_groups(
        &partition(&[(GROUP_A, &["A-1", "A-2"]), (GROUP_B, &["B-1"])]),
        &tasks,
    );
    let menu = roster.menu(&fixture_stamp("A-2"));
    assert_eq!(
        menu.new_group,
        moving(fixture_stamp("A-2"), Destination::New)
    );
    assert_eq!(
        menu.leave,
        moving(fixture_stamp("A-2"), Destination::Ungrouped)
    );
    let menu = roster.menu(&fixture_stamp("B-1"));
    assert_eq!(menu.new_group, None);
    assert_eq!(
        menu.leave,
        moving(fixture_stamp("B-1"), Destination::Ungrouped)
    );
    let menu = roster.menu(&fixture_stamp("loose"));
    assert_eq!((menu.new_group, menu.leave), (None, None));
}
