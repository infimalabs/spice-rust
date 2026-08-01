use super::*;

#[test]
fn operator_trust_process_binds_policy_bytes_across_reconnect_and_restart() {
    let fixture = Fixture::new("trust-lifecycle");
    write_policy(&fixture, "# tracked executable policy v1\n");

    let (first, first_digest) = start_and_grant_initial_policy(&fixture);
    let restarted = restart_with_unchanged_policy(&fixture, first, &first_digest);
    let (changed, changed_digest) = restart_with_changed_policy(&fixture, restarted, &first_digest);

    assert_non_current_digests_refuse(&fixture, &first_digest);
    grant_then_revoke_current_digest(&fixture, &changed_digest);

    assert!(invoke(&fixture, "down").status.success());
    assert!(changed.wait().status.success());
    journal::JournalReader::open(fixture.journal())
        .expect("final authority opens")
        .verify()
        .expect("final trust authority verifies");
}

fn write_policy(fixture: &Fixture, tracked: &str) {
    fs::write(fixture.root.join("spice.toml"), tracked).expect("tracked policy writes");
    let local = fixture.root.join(".git/.spice/config.toml");
    fs::create_dir_all(local.parent().expect("local policy parent"))
        .expect("local policy directory");
    if !local.exists() {
        fs::write(&local, "# local executable policy v1\n").expect("local policy writes");
    }
}

fn start_and_grant_initial_policy(fixture: &Fixture) -> (RunningResident, String) {
    let mut resident = RunningResident::spawn(fixture);
    resident.assert_ready(fixture);
    let initial_pends = trust_pended(fixture);
    assert_eq!(initial_pends.len(), 1, "startup pends exactly once");
    assert_eq!(initial_pends[0].surface, "configuration");
    assert_eq!(initial_pends[0].previous, None);

    let text_report = invoke_args(fixture, &["trust"], false);
    assert!(
        text_report.status.success(),
        "text trust failed: {:?}",
        text_report.stderr
    );
    assert!(text_report.stderr.is_empty());
    let body: serde_json::Value =
        serde_json::from_slice(&text_report.stdout).expect("text trust body is canonical JSON");
    assert_eq!(body["standing"], "pended");
    assert_eq!(body["provenance"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["provenance"][0]["source"], "tracked");
    assert_eq!(body["provenance"][1]["source"], "local");
    assert_eq!(body["provenance"][0]["present"], true);
    assert_eq!(body["provenance"][1]["present"], true);
    let digest = body["digest"]
        .as_str()
        .expect("trust report names its current digest")
        .to_owned();
    assert_eq!(digest, initial_pends[0].digest.as_str());

    grant_initial_digest(fixture, &digest);
    assert_granted_report(fixture);
    (resident, digest)
}

fn grant_initial_digest(fixture: &Fixture, digest: &str) {
    let granted = invoke_args(fixture, &["trust", "grant", digest], true);
    assert!(
        granted.status.success(),
        "JSON grant failed: {:?}",
        granted.stderr
    );
    let Outcome::Reply(reply) = value_outcome(&granted) else {
        panic!("grant must return one typed reply");
    };
    assert!(matches!(reply.position, ReplyPosition::Committed(_)));
    assert_eq!(reply.body["digest"], digest);
    assert_eq!(reply.body["standing"], "granted");

    let before = journal_cursor(fixture);
    let duplicate = invoke_args(fixture, &["trust", "grant", digest], false);
    assert_eq!(duplicate.status.code(), Some(REFUSAL_EXIT));
    assert_eq!(
        duplicate.stderr,
        concat!(
            "spice: trust::standing-moved; re-read the current trust report ",
            "before issuing another ruling\n"
        )
        .as_bytes()
    );
    assert_eq!(journal_cursor(fixture), before);
}

fn assert_granted_report(fixture: &Fixture) {
    let report = invoke_args(fixture, &["trust"], true);
    assert!(report.status.success());
    let Outcome::Reply(reply) = value_outcome(&report) else {
        panic!("JSON trust must return one typed reply");
    };
    assert!(matches!(reply.position, ReplyPosition::Snapshot(_)));
    assert_eq!(reply.body["standing"], "granted");
}

fn restart_with_unchanged_policy(
    fixture: &Fixture,
    resident: RunningResident,
    digest: &str,
) -> RunningResident {
    assert!(invoke(fixture, "down").status.success());
    assert!(resident.wait().status.success());
    let pends_before_restart = trust_pended(fixture);
    let mut restarted = RunningResident::spawn(fixture);
    restarted.assert_ready(fixture);
    let report = invoke_args(fixture, &["trust"], true);
    let Outcome::Reply(reply) = value_outcome(&report) else {
        panic!("restarted trust must return one typed reply");
    };
    assert_eq!(reply.body["digest"], digest);
    assert_eq!(reply.body["standing"], "granted");
    assert_eq!(
        trust_pended(fixture),
        pends_before_restart,
        "restart cannot duplicate an existing digest pend"
    );
    restarted
}

fn restart_with_changed_policy(
    fixture: &Fixture,
    resident: RunningResident,
    previous: &str,
) -> (RunningResident, String) {
    assert!(invoke(fixture, "down").status.success());
    assert!(resident.wait().status.success());
    write_policy(fixture, "# tracked executable policy v2\n");
    let mut changed = RunningResident::spawn(fixture);
    changed.assert_ready(fixture);
    let pends = trust_pended(fixture);
    assert_eq!(pends.len(), 2, "changed bytes append one new pend");
    assert_eq!(
        pends[1].previous.as_ref().map(wire::Digest::as_str),
        Some(previous)
    );
    let report = invoke_args(fixture, &["trust"], true);
    let Outcome::Reply(reply) = value_outcome(&report) else {
        panic!("changed trust must return one typed reply");
    };
    assert_eq!(reply.body["standing"], "pended");
    let digest = reply.body["digest"]
        .as_str()
        .expect("changed report names current digest")
        .to_owned();
    assert_ne!(digest, previous);
    (changed, digest)
}

fn assert_non_current_digests_refuse(fixture: &Fixture, stale: &str) {
    let unknown =
        wire::Digest::compute(wire::Domain::ConfigExecutableSurface, b"unknown").to_string();
    let cross = wire::Digest::compute(wire::Domain::DistArtifact, b"cross-domain").to_string();
    for (label, digest) in [
        ("stale", stale),
        ("unknown", unknown.as_str()),
        ("cross-domain", cross.as_str()),
    ] {
        let before = journal_cursor(fixture);
        let refused = invoke_args(fixture, &["trust", "grant", digest], true);
        assert_eq!(
            refused.status.code(),
            Some(REFUSAL_EXIT),
            "{label} digest did not refuse"
        );
        let Outcome::Refusal(refusal) = value_outcome(&refused) else {
            panic!("{label} digest must return a typed refusal");
        };
        assert_eq!(refusal.reason, "trust::digest-not-current");
        assert_eq!(journal_cursor(fixture), before, "{label} changed authority");
    }
}

fn grant_then_revoke_current_digest(fixture: &Fixture, digest: &str) {
    let grant = invoke_args(fixture, &["trust", "grant", digest], false);
    assert!(
        grant.status.success(),
        "text grant failed: {:?}",
        grant.stderr
    );
    let body: serde_json::Value =
        serde_json::from_slice(&grant.stdout).expect("text grant body is JSON");
    assert_eq!(body["standing"], "granted");

    let revoked = invoke_args(fixture, &["trust", "revoke", digest], true);
    assert!(
        revoked.status.success(),
        "JSON revoke failed: {:?}",
        revoked.stderr
    );
    let Outcome::Reply(reply) = value_outcome(&revoked) else {
        panic!("revoke must return one typed reply");
    };
    assert_eq!(reply.body["standing"], "revoked");

    let before = journal_cursor(fixture);
    let duplicate = invoke_args(fixture, &["trust", "revoke", digest], true);
    assert_eq!(duplicate.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = value_outcome(&duplicate) else {
        panic!("duplicate revoke must return one typed refusal");
    };
    assert_eq!(refusal.reason, "trust::standing-moved");
    assert_eq!(journal_cursor(fixture), before);
}
