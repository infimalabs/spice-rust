//! A claimant hears each matching open Design once per claim, never one that
//! is settled, its own, or a room it already listens to.

use wire::rooms::DesignNoticed;

use super::*;

const PITCH: &str = "Lanes page retained publications through bounded cursors.";
const MATCHING: &str = "Retained publications page through bounded cursors.";
const TERMS: [&str; 7] = [
    "catch",
    "cursor",
    "lane",
    "page",
    "publication",
    "retained",
    "room",
];

/// FIRST and SECOND seated, each with an occupant born one past its lane.
fn claimant_seats() -> Seats {
    let mut seats = Seats::new();
    for lane in [FIRST, SECOND] {
        seats
            .seat_detached(
                test_uuid(lane),
                test_uuid(lane + 1),
                Box::new(DetachedFixture {
                    dropped: Arc::new(AtomicU64::new(0)),
                }),
            )
            .expect("claimant seats");
    }
    seats
}

impl Lanes {
    fn noticed(&self) -> Vec<DesignNoticed> {
        self.bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .filter_map(|envelope| envelope.fact.read::<DesignNoticed>())
            .collect()
    }

    fn design(&self, stamp: &str, title: &str, description: &str) -> wire::TaskHandle {
        self.design_held(stamp, title, description, None)
    }

    fn design_held(
        &self,
        stamp: &str,
        title: &str,
        description: &str,
        holder: Option<u8>,
    ) -> wire::TaskHandle {
        self.seed_task(
            stamp,
            title,
            Some(description),
            Some(wire::TaskIntent::Design),
            holder.map(test_uuid),
        )
    }

    /// Whether `lane` listens to `room` by the current attention projection.
    fn listens(&self, config: &SurfaceConfig, room: &wire::TaskStamp, lane: u8) -> bool {
        let snapshot = self.bus.task_snapshot().expect("snapshot reads");
        let entries = snapshot
            .tasks()
            .iter()
            .map(|heads| {
                board::Entry::new(
                    heads.statement().body().clone(),
                    heads.workflow().body().clone(),
                    heads.claim().map(|head| head.body().clone()),
                    heads.lease().map(|head| head.body().clone()),
                    &config.projects,
                )
                .expect("fixture entry")
            })
            .collect::<Vec<_>>();
        crate::board_command::project_attention(
            &self.bus,
            &entries,
            snapshot.residency().expect("residency").body(),
            room,
            &[],
            Instant::new(jiff::Timestamp::now()).expect("clock"),
        )
        .expect("attention projects")
        .lanes
        .iter()
        .any(|row| row.lane == test_uuid(lane) && row.listens())
    }
}

#[test]
fn a_claimant_hears_each_matching_open_design_once_per_claim() {
    let lanes = Lanes::new("design-awareness");
    let task = lanes.seed_task(
        "dVrD245Kc",
        "Page room catch-up",
        Some("Bound the cursor of retained publications per lane."),
        None,
        Some(test_uuid(FIRST)),
    );
    let open = lanes.design("dVrD245Kd", "Room catch-up pagination", PITCH);
    let settled = lanes.design("dVrD245Km", "Room catch-up retention", MATCHING);
    lanes.close_room(&settled);
    let unrelated = lanes.design("dVrD245Kf", "Working groups", "Arrange the board.");
    let listened = lanes.design("dVrD245Kg", "Room catch-up invitations", MATCHING);
    let held = lanes.design_held(
        "dVrD245Kh",
        "Room catch-up horizons",
        MATCHING,
        Some(SECOND),
    );
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}#{listened}: join us here"));
    lanes.account(SENDER);
    let _ = lanes.hear(SENDER, test_uuid(SENDER + 1));

    let repository = Repository::discover(&lanes.root).expect("fixture repository resolves");
    let startup = load_startup_surface(&repository).expect("fixture surface loads");
    let mut seats = claimant_seats();
    let mut diagnostics = Vec::new();
    let mut reconcile = || {
        crate::resident::design_awareness::reconcile(
            &lanes.bus,
            &lanes.paths,
            &startup.config,
            &seats,
            &mut diagnostics,
        )
        .expect("awareness reconciles");
    };
    reconcile();
    let noticed = lanes.noticed();
    let stamp = |handle: &wire::TaskHandle| handle.born().expect("fixture handle has a birth");
    let told_first = noticed
        .iter()
        .filter(|row| row.lane == test_uuid(FIRST))
        .collect::<Vec<_>>();
    assert_eq!(
        told_first
            .iter()
            .map(|row| row.design.clone())
            .collect::<Vec<_>>(),
        [stamp(&open), stamp(&held)],
        "open Designs the lane does not hold or hear are told, even one another lane \
         holds; not {settled}, {unrelated}, or {listened}"
    );
    let row = told_first[0];
    assert_eq!(row.task, stamp(&task));
    assert_eq!(row.occupant, test_uuid(FIRST + 1));
    assert_eq!(row.terms, TERMS);
    assert!(
        noticed
            .iter()
            .all(|row| !(row.lane == test_uuid(SECOND) && row.design == stamp(&held))),
        "a lane is never told about the Design it holds"
    );

    reconcile();
    assert_eq!(
        lanes.noticed(),
        noticed,
        "a told Design is not told again during the same claim"
    );
    assert!(
        diagnostics.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostics)
    );
    let read = lanes.room_read(&["awareness", open.as_str()]);
    let rows = read["page"]["rows"].as_array().expect("awareness rows");
    assert_eq!(rows[0]["task"], serde_json::json!(task));
    assert_eq!(
        rows.iter()
            .map(|row| (row["lane"].clone(), row["terms"].clone()))
            .collect::<Vec<_>>(),
        noticed
            .iter()
            .filter(|told| told.design == stamp(&open))
            .map(|told| (serde_json::json!(told.lane), serde_json::json!(told.terms)))
            .collect::<Vec<_>>(),
        "the operator reads every lane told about the Design with its matched terms"
    );
    let read = lanes.room_read(&["awareness", unrelated.as_str()]);
    assert_eq!(read["page"]["rows"], serde_json::json!([]));
    assert!(
        !lanes.listens(&startup.config, &stamp(&open), FIRST),
        "being told adds no room attention"
    );
    seats.release_all();
    lanes.finish();
}
