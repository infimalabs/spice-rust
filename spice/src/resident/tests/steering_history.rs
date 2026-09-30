//! Operator direction history pages whole fanout groups newest first.

use wire::Disposition;
use wire::intent::{
    DIRECTION_HISTORY_QUERY_BYTES_MAX, DirectionGroup, DirectionHistoryAnswer, DirectionWindow,
    SteerHistory,
};

use super::*;

const FIRST: u8 = 2;
const SECOND: u8 = 4;
const THIRD: u8 = 6;

/// Three seated lanes, three operator fanouts, one peer tell between them,
/// and one canceled recipient.
struct Directions {
    root: PathBuf,
    paths: RuntimePaths,
    bus: Bus,
    epoch: CanonicalUuid,
}

impl Directions {
    fn new(tag: &str) -> Self {
        let (root, paths) = resident_fixture(tag);
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let worktree = repository.worktree_root().to_string_lossy().into_owned();
        let surface =
            crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
        let bus =
            Bus::start(journal::Journal::open(&paths.journal).expect("fixture journal opens"))
                .expect("fixture bus starts");
        let epoch = test_uuid(1);
        commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
        for lane in [FIRST, SECOND, THIRD] {
            seed_vendor_lane_in_worktree(
                &bus,
                epoch,
                test_uuid(lane),
                test_uuid(lane + 1),
                &worktree,
                "omp",
            );
        }
        let fixture = Self {
            root,
            paths,
            bus,
            epoch,
        };
        fixture.send(10, FIRST, 30, "Alpha first");
        fixture.send(11, SECOND, 30, "Alpha first");
        fixture.tell(12, FIRST, SECOND, "peer alpha note");
        // Authored instants carry milliseconds; keep each fanout distinct.
        std::thread::sleep(Duration::from_millis(3));
        fixture.send(13, FIRST, 31, "beta second");
        std::thread::sleep(Duration::from_millis(3));
        fixture.send(14, SECOND, 32, "> quoted context\nGamma ALPHA third");
        fixture.send(15, THIRD, 32, "> quoted context\nGamma ALPHA third");
        fixture.cancel(16, test_uuid(11));
        fixture
    }

    fn seat(&self, lane: u8) -> Witness {
        let lane = test_uuid(lane);
        let occupant = current_residency(&self.bus, self.epoch)
            .expect("residency reads")
            .body()
            .lanes
            .iter()
            .find(|row| row.lane == lane)
            .and_then(|row| row.occupant);
        Witness::Residency {
            lane,
            occupant,
            epoch: self.epoch,
        }
    }

    fn admit<B: serde::Serialize>(
        &self,
        kind: CommandKind,
        id: u8,
        actor: Actor,
        expected: Vec<Witness>,
        body: &B,
    ) -> Result<wire::AdmittedCommand, wire::CommandRejection> {
        Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
            epoch: self.epoch,
            kind,
            expected,
            body: serde_json::to_value(body).expect("request encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor,
            epoch: self.epoch,
        })
    }

    fn send(&self, id: u8, to: u8, fanout: u8, message: &str) {
        let send = wire::intent::SteerSend {
            fanout: test_uuid(fanout),
            lane: test_uuid(to),
            message: message.to_owned(),
            attachments: Vec::new(),
        };
        let command = self
            .admit(
                CommandKind::SteerSend,
                id,
                Actor::Operator,
                vec![self.seat(to)],
                &send,
            )
            .expect("operator send admits");
        let outcome = steering::send(
            &self.bus,
            self.epoch,
            &steer::Config::default(),
            &command,
            &send,
            &mut CommandEffects::default(),
        );
        assert!(matches!(outcome, Outcome::Reply(_)), "{outcome:?}");
    }

    fn tell(&self, id: u8, from: u8, to: u8, message: &str) {
        let tell = wire::intent::SteerTell {
            lane: test_uuid(to),
            message: message.to_owned(),
        };
        let command = self
            .admit(
                CommandKind::SteerTell,
                id,
                Actor::Lane(test_uuid(from)),
                vec![self.seat(to)],
                &tell,
            )
            .expect("a lane's tell admits");
        let outcome = steering::tell(
            &self.bus,
            self.epoch,
            &steer::Config::default(),
            &command,
            &tell,
            &mut CommandEffects::default(),
        );
        assert!(matches!(outcome, Outcome::Reply(_)), "{outcome:?}");
    }

    fn cancel(&self, id: u8, directive: CanonicalUuid) {
        let cancel = wire::intent::DirectiveRef { directive };
        let witness = Witness::Directive {
            steer: directive,
            disposition: Disposition::Live,
        };
        let command = self
            .admit(
                CommandKind::SteerCancel,
                id,
                Actor::Operator,
                vec![witness],
                &cancel,
            )
            .expect("operator cancel admits");
        let outcome = steering::cancel(
            &self.bus,
            self.epoch,
            &command,
            &cancel,
            &mut CommandEffects::default(),
        );
        assert!(matches!(outcome, Outcome::Reply(_)), "{outcome:?}");
    }

    fn outcome(&self, request: &SteerHistory) -> Outcome<CommandAnswer> {
        let command = self
            .admit(
                CommandKind::SteerHistory,
                90,
                Actor::Operator,
                Vec::new(),
                request,
            )
            .expect("an operator history read admits");
        let Intent::SteerHistory(admitted) = command.body() else {
            panic!("history admits as its own intent");
        };
        steering::history(&self.bus, &self.paths, admitted)
    }

    fn page(&self, request: &SteerHistory) -> DirectionHistoryAnswer {
        match self.outcome(request) {
            Outcome::Reply(Reply {
                body: CommandAnswer::DirectionHistory(answer),
                ..
            }) => *answer,
            outcome => panic!("history reads durable authority: {outcome:?}"),
        }
    }

    fn refusal(&self, request: &SteerHistory) -> String {
        match self.outcome(request) {
            Outcome::Refusal(refusal) => refusal.reason().to_owned(),
            outcome => panic!("history must refuse: {outcome:?}"),
        }
    }

    fn finish(self) {
        self.bus.verify().expect("the authority verifies");
        self.bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(self.root).expect("fixture cleanup");
    }
}

fn read(window: DirectionWindow, limit: u16) -> SteerHistory {
    SteerHistory {
        window,
        query: None,
        lanes: Vec::new(),
        dispositions: Vec::new(),
        authored_before: None,
        limit,
    }
}

fn fanouts(page: &DirectionHistoryAnswer) -> Vec<CanonicalUuid> {
    page.groups.iter().map(|group| group.fanout).collect()
}

/// Fanouts of the newest page after one edit to an otherwise unfiltered read.
fn filtered(fixture: &Directions, edit: impl FnOnce(&mut SteerHistory)) -> Vec<CanonicalUuid> {
    let mut request = read(DirectionWindow::Newest, 10);
    edit(&mut request);
    fanouts(&fixture.page(&request))
}

/// Refusal reason of an otherwise unfiltered read after one edit.
fn refused(fixture: &Directions, edit: impl FnOnce(&mut SteerHistory)) -> String {
    let mut request = read(DirectionWindow::Newest, 10);
    edit(&mut request);
    fixture.refusal(&request)
}

fn recipients(group: &DirectionGroup) -> Vec<(CanonicalUuid, Disposition, bool)> {
    group
        .recipients
        .iter()
        .map(|recipient| {
            (
                recipient.directive,
                recipient.disposition,
                recipient.retirement.is_some(),
            )
        })
        .collect()
}

#[test]
fn history_pages_whole_operator_fanouts_newest_first_with_cursors_both_ways() {
    let fixture = Directions::new("direction-history-pages");
    let [gamma, beta, alpha] = [32, 31, 30].map(test_uuid);

    let newest = fixture.page(&read(DirectionWindow::Newest, 2));
    assert_eq!(fanouts(&newest), [gamma, beta]);
    assert_eq!(newest.total, 3, "the peer tell is not operator direction");
    assert_eq!(newest.newer, None);
    assert_eq!(newest.older, Some(newest.groups[1].position));
    assert_eq!(
        newest.groups[0].preview.as_deref(),
        Some("Gamma ALPHA third")
    );
    assert_eq!(
        recipients(&newest.groups[0]),
        [
            (test_uuid(14), Disposition::Live, false),
            (test_uuid(15), Disposition::Live, false),
        ]
    );

    let older = fixture.page(&read(
        DirectionWindow::Before {
            position: newest.older.expect("an older page exists"),
        },
        2,
    ));
    assert_eq!(fanouts(&older), [alpha]);
    assert_eq!(
        recipients(&older.groups[0]),
        [
            (test_uuid(10), Disposition::Live, false),
            (test_uuid(11), Disposition::Canceled, true),
        ],
        "a fanout arrives whole, with each recipient's own standing"
    );
    assert_eq!(older.older, None);
    let alpha_position = older.groups[0].position;
    assert_eq!(older.newer, Some(alpha_position));

    let newer = fixture.page(&read(
        DirectionWindow::After {
            position: alpha_position,
        },
        1,
    ));
    assert_eq!(
        fanouts(&newer),
        [beta],
        "the nearest newer group comes first"
    );
    assert_eq!(newer.older, Some(newer.groups[0].position));
    assert_eq!(newer.newer, Some(newer.groups[0].position));

    let by_directive = fixture.page(&read(
        DirectionWindow::Anchor {
            identity: test_uuid(15),
        },
        1,
    ));
    assert_eq!(fanouts(&by_directive), [gamma]);
    assert_eq!(by_directive.newer, None);
    assert!(by_directive.older.is_some());
    let by_fanout = fixture.page(&read(DirectionWindow::Anchor { identity: beta }, 5));
    assert_eq!(fanouts(&by_fanout), [beta, alpha]);
    assert_eq!(by_fanout.newer, Some(by_fanout.groups[0].position));
    assert_eq!(by_fanout.older, None);

    assert_eq!(
        fixture.refusal(&read(
            DirectionWindow::Anchor {
                identity: test_uuid(12),
            },
            5,
        )),
        "steer::history-anchor-unknown",
        "a peer tell is never an operator direction anchor"
    );
    fixture.finish();
}

#[test]
fn history_filters_keep_or_drop_each_group_whole_before_paging() {
    let fixture = Directions::new("direction-history-filters");
    let [gamma, beta, alpha] = [32, 31, 30].map(test_uuid);
    let all = fixture.page(&read(DirectionWindow::Newest, 10));

    assert_eq!(
        filtered(&fixture, |request| request.query =
            Some("  ALPHA ".to_owned())),
        [gamma, alpha],
        "the query folds ASCII case and never reaches the peer tell"
    );
    assert_eq!(
        filtered(&fixture, |request| request.query = Some(" \t ".to_owned())),
        [gamma, beta, alpha]
    );
    assert_eq!(
        filtered(&fixture, |request| request.lanes = vec![test_uuid(THIRD)]),
        [gamma]
    );
    let mut by_first = read(DirectionWindow::Newest, 10);
    by_first.lanes = vec![test_uuid(FIRST)];
    let first_lane = fixture.page(&by_first);
    assert_eq!(fanouts(&first_lane), [beta, alpha]);
    assert_eq!(
        recipients(&first_lane.groups[1]),
        [
            (test_uuid(10), Disposition::Live, false),
            (test_uuid(11), Disposition::Canceled, true),
        ],
        "a lane filter keeps the other lane's recipient in the group"
    );
    assert_eq!(
        filtered(&fixture, |request| request.dispositions =
            vec![Disposition::Canceled]),
        [alpha]
    );
    assert_eq!(
        filtered(&fixture, |request| request.dispositions =
            vec![Disposition::Live]),
        [gamma, beta, alpha],
        "a waiting recipient keeps a partly retired group"
    );
    let beta_authored = all.groups[1].authored;
    assert_eq!(
        filtered(&fixture, |request| request.authored_before =
            Some(beta_authored)),
        [beta, alpha]
    );
    let mut paged = read(DirectionWindow::Newest, 1);
    paged.lanes = vec![test_uuid(SECOND)];
    let first = fixture.page(&paged);
    assert_eq!(
        (fanouts(&first), first.total),
        (vec![gamma], 2),
        "filters apply before the page bound"
    );
    paged.window = DirectionWindow::Before {
        position: first.older.expect("a filtered older page exists"),
    };
    assert_eq!(fanouts(&fixture.page(&paged)), [alpha]);

    assert_eq!(
        refused(&fixture, |request| request.limit = 0),
        "steer::history-limit-invalid"
    );
    assert_eq!(
        refused(&fixture, |request| request.limit = 51),
        "steer::history-limit-invalid"
    );
    assert_eq!(
        refused(&fixture, |request| {
            request.query = Some("a".repeat(DIRECTION_HISTORY_QUERY_BYTES_MAX + 1));
        }),
        "steer::history-query-invalid"
    );
    assert_eq!(
        refused(&fixture, |request| request.query =
            Some("al\u{7}pha".to_owned())),
        "steer::history-query-invalid"
    );
    fixture.finish();
}

#[test]
fn history_is_operator_only_and_refuses_an_operator_head_without_a_fanout() {
    let fixture = Directions::new("direction-history-scope");
    let rejection = fixture
        .admit(
            CommandKind::SteerHistory,
            91,
            Actor::Lane(test_uuid(FIRST)),
            Vec::new(),
            &read(DirectionWindow::Newest, 10),
        )
        .expect_err("a lane principal cannot read operator direction history");
    assert!(
        matches!(rejection, wire::CommandRejection::Refusal(_)),
        "{rejection:?}"
    );

    let connection =
        rusqlite::Connection::open(&fixture.paths.journal).expect("fixture journal opens");
    connection
        .execute(
            "UPDATE steering_heads SET body = CAST(json_remove(CAST(body AS TEXT), '$.fanout') \
             AS BLOB) WHERE directive = ?1",
            [test_uuid(13).to_string()],
        )
        .expect("fixture head loses its fanout");
    drop(connection);
    match fixture.outcome(&read(DirectionWindow::Newest, 10)) {
        Outcome::Fault(fault) => assert_eq!(fault.reason, "steer::history-fanout-missing"),
        outcome => panic!("a fanout-less operator head is corrupt: {outcome:?}"),
    }
    fixture.bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(fixture.root).expect("fixture cleanup");
}
