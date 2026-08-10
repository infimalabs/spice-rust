/// A task command whose transaction appends no fact is refused, not receipted.
///
/// A journal write can carry a command receipt without carrying a fact, because
/// the receipt is assembled without one. That shape is unrepresentable: a
/// receipt records the span of facts it caused, and a write with no fact has no
/// span to name, so the commit path reaches for a first and last position that
/// are not there. Removing this refusal does not make such a write succeed
/// quietly -- it aborts the process inside position assignment. What the
/// refusal buys is a typed refusal a producer can report at the door instead of
/// a panic underneath it, and it has to come from the transaction itself,
/// because every producer assembles its receipt before it can know whether its
/// facts survived the guards.
///
/// The witness positions are fixture values. Command law admits a witness list
/// by kind rather than by position, and this transaction is refused before any
/// guard reads, so nothing here depends on which position they name.
#[test]
fn a_task_command_that_appends_no_fact_is_refused_instead_of_receipted() {
    let mut journal = claimed();
    let before = journal.fact_snapshot().expect("facts before").facts().len();
    let holder = Actor::Lane(uuid(20));
    let head = wire::Seq::new(2).expect("fixture head position");

    let mut write = JournalWrite::new(
        holder.clone(),
        "task:BOARD-a1".parse::<Origin>().expect("origin"),
        instant(10),
        Producer::BusTimer,
    );
    write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: uuid(34),
                epoch: uuid(22),
                kind: CommandKind::ClaimRenew,
                expected: vec![
                    Witness::Claim {
                        task: born(),
                        head: Some(head),
                    },
                    Witness::Lease { task: born(), head },
                    Witness::Residency {
                        lane: uuid(20),
                        occupant: uuid(21),
                        epoch: uuid(22),
                    },
                ],
                body: intent::ClaimRenew {
                    task: task(),
                    lease_seconds: 15,
                },
            },
            uuid(22),
            intent::ClaimAnswer {
                task: task(),
                holder,
                warnings: Vec::new(),
                next: None,
            },
        )
        .expect("the receipt itself is well formed");

    let error = journal
        .apply(write)
        .expect_err("a transaction appending no fact cannot apply");
    assert!(
        error.to_string().contains("must append at least one fact"),
        "a factless transaction was refused for another reason: {error}",
    );
    assert_eq!(
        journal.fact_snapshot().expect("facts after").facts().len(),
        before,
        "a refused transaction appended a fact",
    );
}

struct Store {
    path: PathBuf,
}

impl Store {
    fn new() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "spice-board-lapse-{}-{}.sqlite",
                std::process::id(),
                uuid(60)
            )),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        for path in [
            self.path.clone(),
            self.path.with_extension("sqlite-wal"),
            self.path.with_extension("sqlite-shm"),
        ] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove lapse fixture: {error}"),
            }
        }
    }
}
