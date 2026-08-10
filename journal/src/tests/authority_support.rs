fn insert_fact(journal: &Journal, envelope: Envelope<AnyFact>) {
    let bytes = wire::encode(&envelope).expect("encode fact fixture");
    let kind = envelope.fact.kind().as_str();
    let seq = i64::try_from(envelope.seq.get()).expect("fixture seq fits SQLite");
    let cause = envelope
        .cause
        .map(Seq::get)
        .map(|cause| i64::try_from(cause).expect("fixture cause fits SQLite"));
    journal
        .connection
        .execute(
            "INSERT INTO fact_envelopes(seq, wire_version, instant, actor, origin, cause, kind, envelope) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                seq, envelope.v.0, envelope.at.to_string(), envelope.actor.to_string(),
                envelope.origin.as_str(), cause, kind, bytes,
            ],
        )
        .expect("insert fact fixture");
}

fn fact(
    seq: u64,
    cause: Option<u64>,
    kind: FactKind,
    body: serde_json::Value,
) -> Envelope<AnyFact> {
    let mut fields = body.as_object().cloned().expect("a fact body is an object");
    fields.insert(
        "kind".to_owned(),
        serde_json::to_value(kind).expect("a fact kind encodes"),
    );
    let bytes = serde_json::to_vec(&fields).expect("fact fields encode");
    let fact = wire::decode(&bytes).expect("fact fields match the registered body");
    Envelope {
        v: WireVersion::CURRENT,
        seq: Seq::new(seq).expect("nonzero fixture seq"),
        at: instant(),
        actor: Actor::Spice,
        origin: "task:1kKRRsDP".parse::<Origin>().expect("fixture origin"),
        cause: cause.map(|cause| Seq::new(cause).expect("nonzero fixture cause")),
        fact,
    }
}

fn insert_heads(journal: &Journal) {
    insert_head(
        &journal.connection,
        "task_heads",
        "stamp",
        "1kKRRsDP",
        1,
        &task_head(),
    );
    insert_head(
        &journal.connection,
        "workflow_heads",
        "stamp",
        "1kKRRsDP",
        2,
        &workflow_head(),
    );
    insert_head(
        &journal.connection,
        "claim_heads",
        "stamp",
        "1kKRRsDP",
        2,
        &claim_head(),
    );
    journal
        .connection
        .execute(
            "INSERT INTO claim_index(stamp, actor, disposition, head_seq) \
             VALUES ('1kKRRsDP', 'spice', 'active', 2)",
            [],
        )
        .expect("insert claim index fixture");
    insert_head(
        &journal.connection,
        "lease_heads",
        "stamp",
        "1kKRRsDP",
        2,
        &lease_head(),
    );
    insert_head(
        &journal.connection,
        "witness_heads",
        "name",
        "review:JOURNAL-a1",
        2,
        &witness_head(),
    );
    let trust_digest = digest("a");
    insert_head(
        &journal.connection,
        "trust_heads",
        "digest",
        trust_digest.as_str(),
        2,
        &TrustHead {
            digest: trust_digest.clone(),
            state: TrustState::Pended,
        },
    );
    insert_head(
        &journal.connection,
        "finding_heads",
        "identity",
        "gate:fixture",
        2,
        &FindingHead {
            identity: "gate:fixture".to_owned(),
            severity: "error".to_owned(),
            locus: "journal:fixture".to_owned(),
            owner: "journal".to_owned(),
            receipt: digest("b"),
            state: FindingState::Open,
        },
    );
    let transcript = transcript_source_head();
    insert_head(
        &journal.connection,
        "transcript_source_heads",
        "stream",
        transcript.key().as_str(),
        2,
        &transcript,
    );
    journal
        .connection
        .execute(
            "INSERT INTO residency_head(singleton, head_seq, body) VALUES (1, 2, ?1)",
            params![wire::encode(&residency_head()).expect("encode residency")],
        )
        .expect("insert residency head");
}

fn insert_head<T: serde::Serialize>(
    connection: &Connection,
    table: &str,
    key_column: &str,
    key: &str,
    seq: u64,
    head: &T,
) {
    let sql = format!("INSERT INTO {table}({key_column}, head_seq, body) VALUES (?1, ?2, ?3)");
    let seq = i64::try_from(seq).expect("fixture head seq fits SQLite");
    connection
        .execute(
            &sql,
            params![key, seq, wire::encode(head).expect("encode head")],
        )
        .expect("insert head fixture");
}

fn insert_receipt(journal: &Journal) {
    let id = uuid("018f82d4-6f59-7d06-92e3-bce9131163c4");
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");
    let command = Command {
        v: WireVersion::CURRENT,
        id,
        epoch,
        kind: CommandKind::TaskDone,
        expected: Vec::new(),
        body: json!({"task": "JOURNAL-a1", "validation": "fixture"}),
    };
    let response = Response {
        v: WireVersion::CURRENT,
        id,
        epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: Seq::new(1).expect("first seq"),
                last: Seq::new(2).expect("last seq"),
            }),
            body: json!({"task": "JOURNAL-a1", "status": "review"}),
        }),
    };
    journal.connection.execute(
        "INSERT INTO command_receipts(command_id, actor, command_kind, request, evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) VALUES (?1, 'operator', 'task::done', ?2, ?3, ?4, 1, 2, NULL, ?5)",
        params![
            id.to_string(), command.canonical_json().expect("canonical request").into_bytes(),
            wire::encode(&Vec::<Witness>::new()).expect("encode witnesses"),
            wire::encode(&response).expect("encode response"), epoch.to_string(),
        ],
    ).expect("insert command receipt");
}

fn task_head() -> TaskHead {
    TaskHead {
        born: "1kKRRsDP".parse().expect("task stamp"),
        queue_origin: Seq::new(1).expect("queue origin"),
        title: "Lay journal baseline".to_owned(),
        description: None,
        acceptance: Vec::new(),
        project: "journal".to_owned(),
        tags: vec!["journal".to_owned()],
        dependencies: Vec::new(),
        provenance: "task:1kKRRsDP".parse().expect("provenance"),
        revision: Seq::new(1).expect("revision"),
    }
}

fn workflow_head() -> WorkflowHead {
    WorkflowHead {
        flow: vec![phase("todo"), phase("review")],
        phase: phase("review"),
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    }
}

fn claim_head() -> ClaimHead {
    ClaimHead {
        actor: Actor::Spice,
        fact: Seq::new(2).expect("claim fact"),
        claimed: Seq::new(2).expect("claim origin"),
        cell: wire::ClaimCell {
            project: "journal".to_owned(),
            phase: phase("review"),
        },
        lane: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
        occupant: uuid("018f82d4-6f59-7d06-92e3-bce913116303"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116304"),
        disposition: ClaimDisposition::Active,
    }
}

fn lease_head() -> LeaseHead {
    LeaseHead {
        start: instant(),
        renewal: instant(),
        maximum: instant(),
        expiry: instant(),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116304"),
    }
}

fn witness_head() -> WitnessHead {
    WitnessHead {
        name: "review:JOURNAL-a1".to_owned(),
        kind: WitnessClass::Review,
        predicate: "workflow is review".to_owned(),
        head: Seq::new(2).expect("witness head"),
    }
}

fn transcript_source_head() -> TranscriptSourceHead {
    TranscriptSourceHead {
        lane: uuid("018f82d4-6f59-7d06-92e3-bce913116305"),
        vendor: "claude".parse().expect("canonical fixture vendor"),
        stream: "session".to_owned(),
        source: uuid("018f82d4-6f59-7d06-92e3-bce913116306"),
        next: TranscriptPosition {
            member: 1,
            offset: 4096,
        },
    }
}

fn residency_head() -> ResidencyHead {
    ResidencyHead {
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116304"),
        state: ResidentState::Seated,
        topology_revision: 0,
        lanes: vec![LaneResidency {
            lane: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
            lineage_id: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
            name: None,
            worktree: "/worktrees/lane-a".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: Some(uuid("018f82d4-6f59-7d06-92e3-bce913116303")),
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            disposition: "active".to_owned(),
        }],
        subscriptions: vec![wire::LineageSubscription {
            lineage_id: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
            policy: wire::intent::LineageLife::Drive,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        }],
    }
}

fn schema_fixture(
    version: u32,
    state: &str,
    from: Option<u32>,
    to: Option<u32>,
    digest: Option<&str>,
) -> Connection {
    let connection = Connection::open_in_memory().expect("open schema fixture");
    connection.execute_batch(
        "CREATE TABLE journal_schema(singleton INTEGER PRIMARY KEY, version INTEGER NOT NULL, state TEXT NOT NULL, from_version INTEGER, to_version INTEGER, plan_digest TEXT);",
    ).expect("create schema fixture");
    connection.execute(
        "INSERT INTO journal_schema(singleton, version, state, from_version, to_version, plan_digest) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        params![version, state, from, to, digest],
    ).expect("insert schema fixture");
    connection
        .pragma_update(None, "user_version", version)
        .expect("set fixture version");
    connection
}

fn schema_file_fixture(
    path: &Path,
    version: u32,
    state: &str,
    from: Option<u32>,
    to: Option<u32>,
    digest: Option<&str>,
) -> Connection {
    let connection = Connection::open(path).expect("open schema file fixture");
    connection.execute_batch(
        "CREATE TABLE journal_schema(singleton INTEGER PRIMARY KEY, version INTEGER NOT NULL, state TEXT NOT NULL, from_version INTEGER, to_version INTEGER, plan_digest TEXT);",
    ).expect("create schema file fixture");
    connection.execute(
        "INSERT INTO journal_schema(singleton, version, state, from_version, to_version, plan_digest) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        params![version, state, from, to, digest],
    ).expect("insert schema file fixture");
    connection
        .pragma_update(None, "user_version", version)
        .expect("set fixture version");
    connection
}

fn directory_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(root)
        .expect("read schema scratch")
        .map(|entry| {
            let path = entry.expect("read schema scratch entry").path();
            let relative = path
                .strip_prefix(root)
                .expect("entry stays below scratch")
                .to_path_buf();
            let bytes = fs::read(&path).expect("read schema scratch file");
            (relative, bytes)
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct FileState {
    bytes: Vec<u8>,
    length: u64,
    modified: std::time::SystemTime,
}

impl FileState {
    fn read(path: &Path) -> Self {
        let metadata = fs::metadata(path).expect("read fixture metadata");
        Self {
            bytes: fs::read(path).expect("read fixture bytes"),
            length: metadata.len(),
            modified: metadata.modified().expect("read modification instant"),
        }
    }
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        for _ in 0..1024 {
            let candidate = std::env::temp_dir().join(format!(
                "spice-journal-readonly-{}-{}",
                std::process::id(),
                NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => return Self { path: candidate },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create scratch {}: {error}", candidate.display()),
            }
        }
        panic!("could not create a distinct read-only journal scratch directory");
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).expect("remove read-only journal scratch directory");
    }
}

fn directory_entries(path: &Path) -> Vec<String> {
    let mut entries = fs::read_dir(path)
        .expect("read scratch directory")
        .map(|entry| {
            entry
                .expect("read scratch entry")
                .file_name()
                .into_string()
                .expect("scratch entry is UTF-8")
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn authority_tables(journal: &Journal) -> BTreeSet<String> {
    let mut statement = journal.connection.prepare(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    ).expect("query authority tables");
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("map table rows")
        .map(|row| row.expect("read table name"))
        .collect()
}

fn expected_tables() -> BTreeSet<String> {
    [
        "claim_heads",
        "claim_index",
        "command_receipts",
        "cursor_head",
        "fact_envelopes",
        "finding_heads",
        "journal_schema",
        "lease_heads",
        "residency_head",
        "steering_heads",
        "steering_index",
        "steering_misses",
        "task_heads",
        "thread_index",
        "transcript_source_heads",
        "trust_heads",
        "witness_heads",
        "workflow_heads",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Remove indexes introduced after schema 6 when constructing an older authority.
fn drop_steering_index(connection: &Connection) {
    connection
        .execute_batch(
            "DROP TABLE IF EXISTS claim_index;
             DROP TABLE steering_index;
             DROP TABLE steering_misses;
             DROP TABLE steering_heads;",
        )
        .expect("remove post-schema-6 indexes from older fixture");
}

fn admit_empty_steering_fold(journal: &mut Journal) {
    let snapshot = journal.fact_snapshot().expect("migration snapshot");
    assert!(
        snapshot.facts().iter().all(|envelope| {
            crate::guard::steering_fact_identity(&envelope.fact).is_none()
                && envelope.fact.kind() != FactKind::SteerMissed
        }),
        "only a fixture with no steering history may publish an empty fold"
    );
    journal
        .rebuild_steering_index(snapshot.horizon(), &[], &[])
        .expect("publish migrated empty steering fold");
}

fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("canonical fixture uuid")
}

fn digest(nibble: &str) -> Digest {
    nibble.repeat(64).parse().expect("canonical digest")
}

fn instant() -> Instant {
    "2030-04-12T18:44:09.271Z".parse().expect("fixture instant")
}

/// One millisecond before [`instant`], the finest step the spelling admits.
fn earlier() -> Instant {
    "2030-04-12T18:44:09.270Z".parse().expect("earlier instant")
}

fn phase(text: &str) -> Phase {
    text.parse().expect("fixture phase")
}
