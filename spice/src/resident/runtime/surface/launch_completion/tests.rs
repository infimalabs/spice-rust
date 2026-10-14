use crate::resident::lane_surface::LaunchFixture;
use journal::{FactCause, JournalWrite};
use wire::{Actor, Producer};

use super::*;

#[test]
fn a_settled_launch_commits_before_dark_and_then_kicks_its_lane_inbox() {
    let fixture = LaunchFixture::new();
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    seed_vacant_lane(&bus, &fixture);
    send_steering(&bus, &fixture);
    let repository = Repository::discover(&fixture.root).expect("fixture repository resolves");
    let startup = crate::resident::load_startup_surface(&repository)
        .expect("fixture startup policy resolves");
    let capabilities = serve::axum::Capabilities::default();
    let discovery = serve::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: fixture.capability,
        epoch: fixture.epoch,
    };
    crate::resident::steering::publish_lane_surface(
        &fixture.paths,
        &discovery,
        fixture.lane,
        fixture.capability,
    )
    .expect("fixture lane surface publishes");
    let reads = read_materializations::CurrentReads::start_queueing(&bus, &fixture.paths);
    let command_surface = command_surface::Surface::new(
        &bus,
        &repository,
        &fixture.paths,
        fixture.epoch,
        runtimes(&capabilities, &discovery, &fixture.reporter(), &reads),
        &startup.config,
        &reads,
    );
    let door_path = crate::resident::supervisor::command::kick_path(&fixture.paths, fixture.lane);
    let _ = std::fs::remove_file(&door_path);
    let door = std::os::unix::net::UnixDatagram::bind(&door_path).expect("the lane's door binds");
    door.set_nonblocking(true).expect("a door that never waits");
    let kicked = || {
        let mut kick = [0u8; 1024];
        door.recv(&mut kick).ok().map(|length| {
            serde_json::from_slice::<crate::resident::supervisor::command::Kick>(&kick[..length])
                .expect("a kick")
        })
    };
    let mut seats = Seats::new();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime builds")
        .block_on(async {
            let mut lifecycle = lifecycle::Runtime::new(&startup.config.lane);
            let mut launches = super::super::super::launch_runtime::Runtime::new();
            let mut diagnostic = Vec::new();
            command_surface
                .reconcile_lifecycle(&mut lifecycle, &mut seats, &mut launches, &mut diagnostic)
                .expect("pending steering wakes the lane");
            assert!(
                launches.in_flight(),
                "the wake is still proving its session"
            );
            assert_eq!(lane_row(&bus, &fixture).occupant, None);
            assert_eq!(kicked(), None, "nothing kicks a lane before its start");

            settle_before_dark(
                &command_surface,
                &mut seats,
                &mut lifecycle,
                &mut launches,
                &mut diagnostic,
            )
            .await
            .expect("pending launches settle");

            assert!(!launches.in_flight());
            assert!(seats.occupied(fixture.lane));
            assert_eq!(
                kicked(),
                Some(crate::resident::supervisor::command::Kick::Inbox),
                "the committed start kicks the lane's inbox",
            );
            assert_eq!(kicked(), None, "one start kicks once");
        });
    let occupant = lane_row(&bus, &fixture)
        .occupant
        .expect("the proven occupant committed before dark");
    assert!(matches!(
        crate::resident::shutdown::drive_shutdown(&bus, &fixture.paths, fixture.epoch, &mut seats),
        Outcome::Reply(_)
    ));
    let dark = current_residency(&bus, fixture.epoch).expect("residency reads");
    assert_eq!(dark.body().state, wire::ResidentState::Dark);
    assert_eq!(
        lane_row(&bus, &fixture).occupant,
        Some(occupant),
        "the next generation adopts the occupant from its committed row"
    );
    crate::resident::steering::remove_lane_surface(&fixture.paths, fixture.lane);
    drop(door);
    let _ = std::fs::remove_file(&door_path);
    drop(command_surface);
    drop(reads);
    bus.verify().expect("dark settlement authority verifies");
    bus.shutdown().expect("fixture bus stops");
}

fn lane_row(bus: &Bus, fixture: &LaunchFixture) -> wire::LaneResidency {
    current_residency(bus, fixture.epoch)
        .expect("residency reads")
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == fixture.lane)
        .cloned()
        .expect("fixture lane exists")
}

fn seed_vacant_lane(bus: &Bus, fixture: &LaunchFixture) {
    let mut proposal =
        write(Actor::Spice, fixture.epoch, Producer::CommandDoor).expect("fixture write starts");
    let seated = proposal
        .fact(
            wire::body::UpSeated {
                epoch: fixture.epoch,
            },
            FactCause::Root,
        )
        .expect("fixture resident seats");
    let lane = fixture.lane;
    let row = wire::LaneResidency {
        lane,
        team: lane,
        name: None,
        worktree: fixture.root.display().to_string(),
        life: "drive".to_owned(),
        vendor: fixture.vendor.clone(),
        model: None,
        effort: None,
        fast_mode: false,
        occupant: None,
        lineage: Vec::new(),
        capabilities: vec![fixture.capability],
        session: None,
        disposition: "vacant".to_owned(),
    };
    let epoch = fixture.epoch;
    proposal.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![row],
        subscriptions: vec![wire::TeamSubscription {
            team: lane,
            policy: wire::intent::TeamLife::Steer,
            fast_mode: false,
        }],
        renewals: Vec::new(),
    });
    commit(bus, proposal).expect("fixture residency commits");
}

fn send_steering(bus: &Bus, fixture: &LaunchFixture) {
    let epoch = fixture.epoch;
    let residency = current_residency(bus, epoch).expect("residency reads");
    let at = wire::Instant::new(jiff::Timestamp::now()).expect("steering instant");
    let origin: wire::Origin = format!("resident:{epoch}")
        .parse()
        .expect("steering origin");
    let mut sent = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let body = wire::body::SteerSent {
        from: None,
        room: None,
        directive: "018f82d4-6f59-7d06-92e3-00000000002b"
            .parse()
            .expect("fixture directive"),
        fanout: None,
        key: "1kInbox1".to_owned(),
        lane: fixture.lane,
        text: "deliver me".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: "2099-01-01T00:00:00.000Z".parse().expect("fixture expiry"),
    };
    let pending = sent
        .fact(body.clone(), FactCause::Existing(residency.position()))
        .expect("steering fact is valid");
    sent.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&body, origin, at, seq).expect("steering head is valid")
    });
    commit(bus, sent).expect("pending steering commits");
}
