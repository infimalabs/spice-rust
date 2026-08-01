use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use listen::{Mode, Observation, RetroSelector, Span, Watch, retro};
use tape::Tape;

const CODEX_PROSE: &[u8] =
    br#"{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"working"}}"#;
const CODEX_REASONING: &[u8] =
    br#"{"type":"item.completed","item":{"id":"item-2","type":"reasoning","text":"inspect"}}"#;

#[derive(Debug, Eq, PartialEq)]
struct Entry {
    directory: bool,
    modified: SystemTime,
    bytes: Vec<u8>,
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-listen-surface-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("scratch root is exclusively created");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn remove(self) {
        fs::remove_dir_all(&self.0).expect("scratch root is explicitly removed");
        assert!(!self.0.exists(), "scratch root outlived removal");
        std::mem::forget(self);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn record(bytes: &[u8]) -> Vec<u8> {
    let mut record = bytes.to_vec();
    record.push(b'\n');
    record
}

fn session(root: &Path, lane: &str, dialect: &str) -> (PathBuf, Tape) {
    let path = root.join(lane).join(dialect);
    let tape = Tape::open(&path).expect("fixture tape opens");
    (path, tape)
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn walk(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
        let mut children = fs::read_dir(path)
            .expect("snapshot directory reads")
            .collect::<Result<Vec<_>, _>>()
            .expect("snapshot entries read");
        children.sort_by_key(std::fs::DirEntry::path);
        for child in children {
            let path = child.path();
            let metadata = child.metadata().expect("snapshot metadata reads");
            let directory = metadata.is_dir();
            let bytes = if directory {
                Vec::new()
            } else {
                fs::read(&path).expect("snapshot file reads")
            };
            entries.insert(
                path.strip_prefix(root)
                    .expect("snapshot path is beneath root")
                    .to_path_buf(),
                Entry {
                    directory,
                    modified: metadata.modified().expect("fixture has modification time"),
                    bytes,
                },
            );
            if directory {
                walk(root, &path, entries);
            }
        }
    }

    let mut entries = BTreeMap::new();
    walk(root, root, &mut entries);
    entries
}

fn event_count(observations: &[Observation]) -> usize {
    observations
        .iter()
        .filter(|observation| matches!(observation, Observation::Event { .. }))
        .count()
}

#[test]
fn watch_heals_missed_wakes_by_comparing_durable_cursors_across_growth_and_sealing() {
    let scratch = Scratch::new("watch");
    let (_, tape) = session(scratch.path(), "lane-a", "codex");
    let mut watch = Watch::new(scratch.path());

    let empty = watch.poll().expect("empty session is observed");
    assert!(matches!(
        empty.observations.as_slice(),
        [Observation::Absent { .. }]
    ));

    tape.tee(&record(CODEX_PROSE))
        .expect("first complete record is durable");
    let first = watch.poll().expect("first cursor delta reads");
    assert_eq!(event_count(&first.observations), 1);
    assert!(first.observations.iter().any(|observation| matches!(
        observation,
        Observation::Canary { expected, .. } if expected == "reasoning"
    )));
    assert!(
        watch
            .poll()
            .expect("an unchanged cursor reconciles")
            .is_empty()
    );

    let complete = record(CODEX_REASONING);
    let split = complete.len() / 2;
    tape.tee(&complete[..split])
        .expect("partial record prefix is durable");
    let partial = watch.poll().expect("partial record is visible");
    assert!(matches!(
        partial.observations.as_slice(),
        [Observation::Pending { .. }]
    ));
    tape.tee(&complete[split..])
        .expect("record suffix is durable without a separate watch wake");
    let completed = watch
        .poll()
        .expect("cursor comparison heals the missed wake");
    assert_eq!(event_count(&completed.observations), 1);

    tape.tee(&record(CODEX_PROSE))
        .expect("first unobserved record is durable");
    tape.tee(&record(CODEX_REASONING))
        .expect("second unobserved record is durable");
    assert_eq!(
        event_count(
            &watch
                .poll()
                .expect("one reconciliation reads both missed records")
                .observations
        ),
        2
    );

    tape.seal().expect("first member seals");
    tape.tee(&record(CODEX_PROSE))
        .expect("next member begins after rotation");
    assert_eq!(
        event_count(
            &watch
                .poll()
                .expect("cursor crosses the sealed member boundary")
                .observations
        ),
        1
    );

    scratch.remove();
}

#[test]
fn retro_accounts_for_partial_malformed_missing_and_ambiguous_spans() {
    let scratch = Scratch::new("retro");
    let (codex, tape) = session(scratch.path(), "lane-a", "codex");
    tape.tee(b"not-json\n")
        .expect("malformed complete record is durable");
    tape.tee(b"{\"type\":\"item.completed\"")
        .expect("partial trailing record is durable");

    let replay = retro(scratch.path(), RetroSelector::Path(codex.clone()))
        .expect("path replay reads preserved tape");
    assert_eq!(replay.mode, Mode::Retro);
    assert!(replay.observations.iter().any(|observation| matches!(
        observation,
        Observation::Residue { reason, .. } if reason.starts_with("malformed:")
    )));
    assert!(replay.observations.iter().any(|observation| matches!(
        observation,
        Observation::Residue { reason, .. } if reason == "truncated"
    )));

    let missing_lane = retro(
        scratch.path(),
        RetroSelector::Lane("lane-missing".to_owned()),
    )
    .expect("missing lane is accounted");
    assert!(matches!(
        missing_lane.observations.as_slice(),
        [Observation::Missing { .. }]
    ));

    let missing_locus = retro(
        scratch.path(),
        RetroSelector::Locus(Span {
            member: 9,
            offset: 0,
            length: 1,
        }),
    )
    .expect("missing locus is accounted");
    assert!(matches!(
        missing_locus.observations.as_slice(),
        [Observation::Missing { .. }]
    ));

    let (_, claude) = session(scratch.path(), "lane-b", "claude");
    claude
        .tee(b"{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n")
        .expect("second session record is durable");
    let ambiguous = retro(
        scratch.path(),
        RetroSelector::Locus(Span {
            member: 1,
            offset: 0,
            length: 1,
        }),
    )
    .expect("ambiguous locus is accounted");
    assert!(matches!(
        ambiguous.observations.as_slice(),
        [Observation::Ambiguous { candidates, .. }] if candidates == &vec![codex, scratch.path().join("lane-b/claude")]
    ));

    scratch.remove();
}

#[test]
fn watch_and_retro_leave_authority_and_discovery_bytes_unchanged() {
    let scratch = Scratch::new("readonly");
    let (codex, tape) = session(scratch.path(), "lane-a", "codex");
    tape.tee(&record(CODEX_PROSE))
        .expect("fixture record is durable");
    for name in ["journal", "claims", "heads", "discovery"] {
        fs::write(scratch.path().join(name), format!("sentinel:{name}\n"))
            .expect("authority sentinel writes");
    }
    let before = snapshot(scratch.path());

    let mut watch = Watch::new(scratch.path());
    assert_eq!(
        event_count(&watch.poll().expect("watch reads").observations),
        1
    );
    let replay = retro(scratch.path(), RetroSelector::Path(codex)).expect("retro reads");
    assert_eq!(event_count(&replay.observations), 1);

    assert_eq!(snapshot(scratch.path()), before);

    let missing = scratch.path().join("never-created/codex");
    let mut missing_watch = Watch::new(&missing);
    assert!(matches!(
        missing_watch
            .poll()
            .expect("missing watch root is accounted")
            .observations
            .as_slice(),
        [Observation::Absent { .. }]
    ));
    assert!(matches!(
        retro(scratch.path(), RetroSelector::Path(missing.clone()))
            .expect("missing retro path is accounted")
            .observations
            .as_slice(),
        [Observation::Missing { .. }]
    ));
    assert!(
        !missing.exists(),
        "observation never creates a selected path"
    );

    scratch.remove();
}

#[test]
fn retro_selector_grammar_never_falls_through_to_another_meaning() {
    assert_eq!(
        RetroSelector::parse("2:17:31").expect("compact locus parses"),
        RetroSelector::Locus(Span {
            member: 2,
            offset: 17,
            length: 31
        })
    );
    assert_eq!(
        RetroSelector::parse("member 2 offset 17 length 31").expect("long locus parses"),
        RetroSelector::Locus(Span {
            member: 2,
            offset: 17,
            length: 31
        })
    );
    assert_eq!(
        RetroSelector::parse("missing/path").expect("path syntax parses despite absence"),
        RetroSelector::Path(PathBuf::from("missing/path"))
    );
    assert_eq!(
        RetroSelector::parse("lane-a").expect("lane parses"),
        RetroSelector::Lane("lane-a".to_owned())
    );
    assert!(RetroSelector::parse("one:two:three").is_err());
    assert!(RetroSelector::parse("1:2").is_err());
    assert!(RetroSelector::parse("1:2:3:4").is_err());
    assert!(RetroSelector::parse("member 2 offset nope length 31").is_err());
}

#[cfg(unix)]
#[test]
fn an_explicit_session_symlink_is_not_discovered_as_preserved_tape() {
    use std::os::unix::fs::symlink;

    let scratch = Scratch::new("symlink");
    let (codex, tape) = session(scratch.path(), "lane-a", "codex");
    tape.tee(&record(CODEX_PROSE))
        .expect("real fixture record is durable");
    let alias = scratch.path().join("lane-b/codex");
    fs::create_dir(alias.parent().expect("alias has a parent")).expect("alias parent exists");
    symlink(&codex, &alias).expect("fixture symlink exists");

    let replay = retro(scratch.path(), RetroSelector::Path(alias))
        .expect("a symlink is accounted without being followed");
    assert!(matches!(
        replay.observations.as_slice(),
        [Observation::Missing { .. }]
    ));

    scratch.remove();
}
