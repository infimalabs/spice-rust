use super::*;

#[test]
fn briefing_authority_probe_is_read_only_when_repository_state_is_absent() {
    let scratch = Scratch::new("authority-absent");
    fake_repository(scratch.path());
    let transcript = scratch.path().join("codex.jsonl");
    write_jsonl(&transcript, &codex_rows());

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&[
            "session",
            "briefing",
            &transcript.to_string_lossy(),
            "--deadline-seconds",
            "5",
        ]),
        &[],
        "recall",
    );

    assert_eq!(recall["authorities"]["fold"]["mode"], "bounded-read-only");
    assert_eq!(recall["authorities"]["fold"]["writes"], json!([]));
    assert!(
        recall["authorities"].get("asks").is_none(),
        "empty outstanding asks are omitted"
    );
    assert!(
        recall["authorities"]["unknowns"]
            .as_array()
            .expect("unknowns are explicit")
            .iter()
            .any(|row| row["authority"] == "journal" && row["probe"] == "open-read-only"),
        "missing journal authority is reported as unknown"
    );
    assert!(
        !scratch.path().join(".git/.spice-rs").exists(),
        "read-only observation does not initialize repository state"
    );

    scratch.remove();
}

#[test]
fn briefing_authority_sources_asks_and_held_work_from_exact_state_authorities() {
    let scratch = Scratch::new("authority-present");
    fake_repository(scratch.path());
    let transcript = scratch.path().join("codex.jsonl");
    write_jsonl(&transcript, &codex_rows());
    let thread = uuid(0xaa).to_string().replace('-', "");
    seed_briefing_authority(&authority_path(scratch.path()), &thread);
    let environment = [("CODEX_THREAD_ID", thread.clone())]; // env-policy: allow

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&[
            "session",
            "briefing",
            &transcript.to_string_lossy(),
            "--start",
            "2026-01-01T00:00:00Z",
            "--max-lines",
            "80",
            "--max-bytes",
            "8000",
            "--deadline-seconds",
            "5",
        ]),
        &environment,
        "recall",
    );

    assert_eq!(recall["authorities"]["asks"][0]["key"], "1kAsk001");
    assert_eq!(
        recall["authorities"]["asks"][0]["text"],
        "Address this exact steering ask"
    );
    assert_eq!(
        recall["authorities"]["held_work"]["own_claim"]["title"],
        "Compose Rust briefings"
    );
    assert_eq!(recall["authorities"]["held_work"]["peer_claims"], 1);
    assert_eq!(
        recall["authorities"]["priority"]["order"],
        json!([
            "outstanding-asks",
            "held-work",
            "transcript-deliveries",
            "transcript-working-set",
            "ambient-posture"
        ])
    );
    assert_eq!(
        recall["window"]["turns"][0]["touched_paths"],
        json!(["spice/src/session.rs"])
    );

    let pruned = reply_forms_in(
        Some(scratch.path()),
        &strings(&[
            "session",
            "briefing",
            &transcript.to_string_lossy(),
            "--start",
            "2026-01-01T00:00:00Z",
            "--max-lines",
            "1",
            "--max-bytes",
            "1",
            "--deadline-seconds",
            "5",
        ]),
        &environment,
        "recall",
    );
    assert!(
        pruned["pruning"]["turns_before"].as_u64() > pruned["pruning"]["turns_after"].as_u64(),
        "end pruning removed transcript turns"
    );
    assert_eq!(pruned["authorities"]["asks"][0]["key"], "1kAsk001");
    assert_eq!(
        pruned["authorities"]["held_work"]["own_claim"]["title"],
        "Compose Rust briefings"
    );
    assert_eq!(
        pruned["authorities"]["priority"]["end_pruning_preserves"],
        json!(["outstanding-asks", "held-work"])
    );

    scratch.remove();
}
