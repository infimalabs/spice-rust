use std::sync::atomic::{AtomicU64, Ordering};

use super::newest_rows;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
const FACTS: u64 = 7;
const INITIAL_SEQUENCE: u64 = 52 * 52 * 52;

fn position(ordinal: u64) -> u64 {
    INITIAL_SEQUENCE + ordinal - 1
}

#[test]
fn a_start_at_the_horizon_reads_from_the_newest_fact() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.page(position(FACTS), 3),
        [position(7), position(6), position(5)]
    );
    assert_eq!(
        fixture.page(position(FACTS), 20),
        (1..=FACTS).rev().map(position).collect::<Vec<_>>()
    );
}

#[test]
fn an_older_start_is_its_page_s_first_row() {
    let fixture = Fixture::new();
    assert_eq!(fixture.page(position(4), 2), [position(4), position(3)]);
    assert_eq!(fixture.page(position(1), 3), [position(1)]);
}

#[test]
fn continuations_visit_every_fact_once_newest_first() {
    let fixture = Fixture::new();
    // The CLI retains one row past its limit and resumes inclusively there.
    let limit = 2;
    let mut start = Some(position(FACTS));
    let mut visited: Vec<u64> = Vec::new();
    while let Some(from) = start {
        let page = fixture.page(from, limit + 1);
        visited.extend(page.iter().take(limit));
        start = page.get(limit).copied();
    }
    assert_eq!(visited, (1..=FACTS).rev().map(position).collect::<Vec<_>>());
}

struct Fixture {
    root: std::path::PathBuf,
    journal: journal::Journal,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-journal-page-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("create journal page scratch");
        let mut journal =
            journal::Journal::open(root.join("authority.sqlite3")).expect("fixture journal opens");
        for offset in 0..FACTS {
            let mut write = journal::JournalWrite::new(
                wire::Actor::Operator,
                format!("taste:page-{offset}")
                    .parse()
                    .expect("fixture origin"),
                "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
                wire::Producer::Judge,
            );
            write
                .fact(
                    wire::body::TasteConfirmed {
                        taste: "fixture".to_owned(),
                        locus: format!("fixture:{offset}"),
                    },
                    journal::FactCause::Root,
                )
                .expect("fixture fact builds");
            journal.apply(write).expect("fixture fact commits");
        }
        Self { root, journal }
    }

    fn page(&self, start: u64, retain: usize) -> Vec<u64> {
        self.journal
            .read(|read| {
                let horizon = read.cursor()?.expect("fixture journal has a cursor");
                assert_eq!(horizon.get(), position(FACTS));
                let start = wire::Seq::new(start).expect("fixture start");
                Ok(newest_rows(read, start, horizon, retain, 64))
            })
            .expect("fixture snapshot reads")
            .unwrap_or_else(|_| panic!("newest-first page from {start} reads"))
            .into_iter()
            .map(|(row, key)| {
                assert_eq!(key, row.seq.to_string());
                row.seq.get()
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove journal page scratch");
    }
}
