use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use listen::{
    Dialect, EventKind, Mode, Observation, RetroSelector, Span, StreamOutcome, SurfaceEvent,
    SurfaceReasoning, SurfaceResidue, SurfaceToolMoment, SurfaceToolResult, Watch, retro,
};
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
        Observation::Canary {
            expected: EventKind::Reasoning,
            ..
        }
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
        Observation::Residue {
            residue: SurfaceResidue::Malformed { .. },
            ..
        }
    )));
    assert!(replay.observations.iter().any(|observation| matches!(
        observation,
        Observation::Residue {
            residue: SurfaceResidue::Truncated {},
            ..
        }
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
        [Observation::Ambiguous { candidates, .. }]
            if candidates == &vec![codex, scratch.path().join("lane-b/claude")]
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

#[test]
fn records_keep_one_typed_identity_across_sealed_member_boundaries() {
    let scratch = Scratch::new("cross-member");
    let (codex, tape) = session(scratch.path(), "lane-a", "codex");
    let complete = record(CODEX_PROSE);
    let first = complete.len() / 3;
    let second = first * 2;

    tape.tee(&complete[..first])
        .expect("first record fragment is durable");
    tape.seal().expect("first fragment member seals");
    tape.tee(&complete[first..second])
        .expect("second record fragment is durable");
    tape.seal().expect("second fragment member seals");
    tape.tee(&complete[second..])
        .expect("record delimiter is durable in the active member");

    let mut watch = Watch::new(scratch.path());
    let watched = watch.poll().expect("watch joins all three fragments");
    let events = watched
        .observations
        .iter()
        .filter_map(|observation| match observation {
            Observation::Event { spans, event, .. } => Some((spans, event)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1.kind(), EventKind::Prose);
    assert_eq!(
        events[0].0,
        &vec![
            Span {
                member: 1,
                offset: 0,
                length: first as u64,
            },
            Span {
                member: 2,
                offset: 0,
                length: (second - first) as u64,
            },
            Span {
                member: 3,
                offset: 0,
                length: (complete.len() - second) as u64,
            },
        ]
    );
    assert!(watched.observations.iter().all(|observation| !matches!(
        observation,
        Observation::Residue { .. } | Observation::Pending { .. }
    )));
    assert!(watch.poll().expect("cursor advances once").is_empty());

    let replay =
        retro(scratch.path(), RetroSelector::Path(codex)).expect("retro joins the same fragments");
    assert_eq!(event_count(&replay.observations), 1);
    assert!(replay.observations.iter().all(|observation| !matches!(
        observation,
        Observation::Residue { .. } | Observation::Pending { .. }
    )));

    scratch.remove();
}

#[test]
fn a_sealed_prefix_remains_pending_until_the_active_member_completes_it() {
    let scratch = Scratch::new("sealed-prefix");
    let (_, tape) = session(scratch.path(), "lane-a", "codex");
    let complete = record(CODEX_REASONING);
    let split = complete.len() / 2;
    tape.tee(&complete[..split])
        .expect("record prefix is durable");
    tape.seal().expect("record prefix seals");

    let mut watch = Watch::new(scratch.path());
    let pending = watch.poll().expect("sealed prefix remains pending");
    assert!(matches!(
        pending.observations.as_slice(),
        [Observation::Pending { spans, .. }] if spans == &vec![Span {
            member: 1,
            offset: 0,
            length: split as u64,
        }]
    ));

    tape.tee(&complete[split..])
        .expect("active member completes the record");
    let completed = watch.poll().expect("completion joins the sealed prefix");
    assert_eq!(event_count(&completed.observations), 1);
    assert!(completed.observations.iter().any(|observation| matches!(
        observation,
        Observation::Event { spans, event, .. }
            if event.kind() == EventKind::Reasoning && spans.len() == 2
    )));
    assert!(completed.observations.iter().all(|observation| !matches!(
        observation,
        Observation::Residue { .. } | Observation::Pending { .. }
    )));

    scratch.remove();
}

#[test]
fn a_newline_before_rotation_keeps_the_next_split_record_distinct() {
    let scratch = Scratch::new("newline-rotation");
    let (_, tape) = session(scratch.path(), "lane-a", "codex");
    let first = record(CODEX_PROSE);
    let second = record(CODEX_REASONING);
    let split = second.len() / 2;
    let mut member = first.clone();
    member.extend_from_slice(&second[..split]);
    tape.tee(&member)
        .expect("whole first record and second prefix are durable");
    tape.seal().expect("mixed member seals");
    tape.tee(&second[split..])
        .expect("second record completes after rotation");

    let mut watch = Watch::new(scratch.path());
    let observed = watch.poll().expect("both records decode once");
    let events = observed
        .observations
        .iter()
        .filter_map(|observation| match observation {
            Observation::Event { spans, event, .. } => Some((spans, event.kind())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].1, EventKind::Prose);
    assert_eq!(events[0].0.len(), 1);
    assert_eq!(events[1].1, EventKind::Reasoning);
    assert_eq!(events[1].0.len(), 2);
    assert!(observed.observations.iter().all(|observation| !matches!(
        observation,
        Observation::Residue { .. } | Observation::Pending { .. }
    )));

    scratch.remove();
}

#[test]
fn finite_retro_accounts_one_terminal_partial_over_every_member() {
    let scratch = Scratch::new("terminal-partial");
    let (codex, tape) = session(scratch.path(), "lane-a", "codex");
    tape.tee(b"{\"type\":")
        .expect("first incomplete fragment is durable");
    tape.seal().expect("first incomplete fragment seals");
    tape.tee(b"\"item.completed\"")
        .expect("second incomplete fragment is durable");

    let replay = retro(scratch.path(), RetroSelector::Path(codex))
        .expect("finite retro accounts the terminal fragment");
    assert!(matches!(
        replay.observations.as_slice(),
        [Observation::Residue {
            spans,
            residue: SurfaceResidue::Truncated {},
            ..
        }] if spans == &vec![
            Span { member: 1, offset: 0, length: 8 },
            Span { member: 2, offset: 0, length: 16 },
        ]
    ));

    scratch.remove();
}

#[test]
fn every_surface_event_has_one_closed_canonical_wire_shape() {
    let events = vec![
        SurfaceEvent::Session {
            identity: "0192f806-a22e-708b-bef1-0573a7d27786".to_owned(),
        },
        SurfaceEvent::Prose {
            text: "hello".to_owned(),
            phase: Some("final".to_owned()),
        },
        SurfaceEvent::Reasoning {
            content: SurfaceReasoning::Plain {
                lines: vec!["inspect".to_owned()],
            },
        },
        SurfaceEvent::Reasoning {
            content: SurfaceReasoning::Opaque {},
        },
        SurfaceEvent::Tool {
            call_id: "call-1".to_owned(),
            moment: SurfaceToolMoment::Call {
                name: "shell".to_owned(),
            },
        },
        SurfaceEvent::Tool {
            call_id: "call-1".to_owned(),
            moment: SurfaceToolMoment::Update {
                name: "shell".to_owned(),
            },
        },
        SurfaceEvent::Tool {
            call_id: "call-1".to_owned(),
            moment: SurfaceToolMoment::Result {
                standing: SurfaceToolResult::Succeeded,
            },
        },
        SurfaceEvent::Tool {
            call_id: "call-2".to_owned(),
            moment: SurfaceToolMoment::Result {
                standing: SurfaceToolResult::Failed,
            },
        },
        SurfaceEvent::Tool {
            call_id: "call-3".to_owned(),
            moment: SurfaceToolMoment::Result {
                standing: SurfaceToolResult::Unreported,
            },
        },
        SurfaceEvent::Usage {
            turns: 1,
            input_tokens: 1,
            cached_input_tokens: 2,
            cache_write_input_tokens: 3,
            output_tokens: 4,
            reasoning_output_tokens: Some(5),
        },
        SurfaceEvent::Compaction {},
        SurfaceEvent::Failure {
            message: "failed".to_owned(),
        },
    ];
    assert_eq!(
        events.iter().map(SurfaceEvent::kind).collect::<Vec<_>>(),
        vec![
            EventKind::Session,
            EventKind::Prose,
            EventKind::Reasoning,
            EventKind::Reasoning,
            EventKind::Tool,
            EventKind::Tool,
            EventKind::Tool,
            EventKind::Tool,
            EventKind::Tool,
            EventKind::Usage,
            EventKind::Compaction,
            EventKind::Failure,
        ]
    );
    for event in events {
        let bytes = wire::encode(&event).expect("surface event encodes");
        let decoded: SurfaceEvent = wire::decode(&bytes).expect("surface event decodes");
        assert_eq!(decoded, event);
        assert_eq!(wire::encode(&decoded).expect("event re-encodes"), bytes);
    }
}

#[test]
fn stream_identity_refuses_missing_extra_and_unknown_nested_shapes() {
    let outcome = StreamOutcome {
        mode: Mode::Watch,
        root: PathBuf::from("root"),
        observations: vec![Observation::Event {
            lane: "lane-a".to_owned(),
            dialect: Dialect::Codex,
            tape: PathBuf::from("root/lane-a/codex"),
            spans: vec![Span {
                member: 1,
                offset: 2,
                length: 3,
            }],
            event: SurfaceEvent::Prose {
                text: "hello".to_owned(),
                phase: None,
            },
        }],
    };
    let bytes = wire::encode(&outcome).expect("stream outcome encodes");
    let canonical: serde_json::Value =
        serde_json::from_slice(&bytes).expect("canonical outcome is JSON");
    assert!(canonical["observations"][0]["event"].get("phase").is_none());
    let decoded: StreamOutcome = wire::decode(&bytes).expect("stream outcome decodes");
    assert_eq!(decoded, outcome);
    assert_eq!(wire::encode(&decoded).expect("outcome re-encodes"), bytes);

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("fixture is JSON");
    value["extra"] = true.into();
    assert!(wire::decode::<StreamOutcome>(&serde_json::to_vec(&value).unwrap()).is_err());

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("fixture is JSON");
    value
        .as_object_mut()
        .expect("outcome is an object")
        .remove("mode");
    assert!(wire::decode::<StreamOutcome>(&serde_json::to_vec(&value).unwrap()).is_err());

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("fixture is JSON");
    value["observations"][0]["spans"][0]["extra"] = true.into();
    assert!(wire::decode::<StreamOutcome>(&serde_json::to_vec(&value).unwrap()).is_err());

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("fixture is JSON");
    value["observations"][0]["event"]["extra"] = true.into();
    assert!(wire::decode::<StreamOutcome>(&serde_json::to_vec(&value).unwrap()).is_err());

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("fixture is JSON");
    value["observations"][0]["event"]
        .as_object_mut()
        .expect("event is an object")
        .remove("text");
    assert!(wire::decode::<StreamOutcome>(&serde_json::to_vec(&value).unwrap()).is_err());

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("fixture is JSON");
    value["observations"][0]["event"]["kind"] = "future".into();
    assert!(wire::decode::<StreamOutcome>(&serde_json::to_vec(&value).unwrap()).is_err());

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("fixture is JSON");
    value["observations"][0]["event"]["phase"] = serde_json::Value::Null;
    assert!(wire::decode::<StreamOutcome>(&serde_json::to_vec(&value).unwrap()).is_err());

    let usage = SurfaceEvent::Usage {
        turns: 1,
        input_tokens: 1,
        cached_input_tokens: 2,
        cache_write_input_tokens: 3,
        output_tokens: 4,
        reasoning_output_tokens: None,
    };
    let usage_bytes = wire::encode(&usage).expect("usage without reasoning tokens encodes");
    let mut usage_value: serde_json::Value =
        serde_json::from_slice(&usage_bytes).expect("canonical usage is JSON");
    assert!(usage_value.get("reasoning_output_tokens").is_none());
    usage_value["reasoning_output_tokens"] = serde_json::Value::Null;
    assert!(wire::decode::<SurfaceEvent>(&serde_json::to_vec(&usage_value).unwrap()).is_err());
}

#[test]
fn every_closed_surface_vocabulary_refuses_unknown_variants() {
    assert!(wire::decode::<Dialect>(br#""future""#).is_err());
    assert!(wire::decode::<EventKind>(br#""future""#).is_err());
    assert!(wire::decode::<Mode>(br#""future""#).is_err());
    assert!(wire::decode::<Observation>(br#"{"type":"future"}"#).is_err());
    assert!(wire::decode::<SurfaceEvent>(br#"{"kind":"future"}"#).is_err());
    assert!(wire::decode::<SurfaceReasoning>(br#"{"form":"future"}"#).is_err());
    assert!(wire::decode::<SurfaceToolMoment>(br#"{"moment":"future"}"#).is_err());
    assert!(wire::decode::<SurfaceToolResult>(br#""future""#).is_err());
    assert!(wire::decode::<SurfaceResidue>(br#"{"reason":"future"}"#).is_err());
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
