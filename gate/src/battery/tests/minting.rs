//! Minting, verifying, and publishing one receipt over one report.

use std::io::Read as _;

use flate2::read::GzDecoder;

use super::*;

#[test]
fn a_minted_receipt_binds_every_identity_of_its_run() {
    let fixture = Fixture::new();
    let baseline = fixture_baseline("published floors");
    let (report, receipt) = minted(&fixture, &baseline);

    assert_eq!(receipt.version(), GATE_BATTERY_RECEIPT_VERSION);
    assert_eq!(
        receipt.manifest(),
        &Digest::compute(
            Domain::GateBatteryManifest,
            CANONICAL_BATTERY_MANIFEST.as_bytes()
        )
    );
    assert_eq!(receipt.gate_set(), report.gate_set());
    assert_eq!(receipt.tree(), &fixture.source);
    assert_eq!(receipt.policy(), &fixture.policy_digest());
    assert_eq!(receipt.state(), report.context().state());
    assert_eq!(receipt.bounds(), report.context().bounds());
    assert_eq!(receipt.baseline(), &baseline);
    assert_eq!(receipt.actor(), report.context().actor());
    assert_eq!(receipt.scope(), report.context().scope());
    assert_eq!(receipt.configuration(), report.context().configuration());
    assert_eq!(receipt.latches(), report.context().latches());
    assert_eq!(receipt.tools(), report.context().tools());
    assert_eq!(
        receipt
            .results()
            .iter()
            .map(BatteryArmResult::gate_id)
            .collect::<Vec<_>>(),
        GateId::BATTERY,
    );
    assert_eq!(
        receipt.report(),
        &Digest::compute(
            Domain::GateBatteryReport,
            &report.encode().expect("report JSON")
        )
    );
    assert_eq!(receipt.run(), &report);
    assert_eq!(
        supported_pairs(),
        [(GATE_BATTERY_RECEIPT_VERSION, receipt.gate_set().clone())],
        "this build must interpret exactly the one pair it mints and no other",
    );

    // The one admitted spelling, pinned exactly: a receipt field added,
    // renamed, retyped, or moved changes these leading bytes, so it changes the
    // receipt's identity instead of quietly passing as the same schema.
    let encoded = receipt.encode().expect("receipt JSON");
    let text = String::from_utf8(encoded).expect("canonical receipts are UTF-8");
    assert!(
        text.starts_with(&format!(
            "{{\"version\":{},\"manifest\":\"{}\",\"baseline\":\"{}\",\"report\":\"{}\",\"run\":{{",
            GATE_BATTERY_RECEIPT_VERSION,
            receipt.manifest(),
            baseline,
            receipt.report(),
        )),
        "receipt schema is not the one admitted spelling: {}",
        &text[..text.len().min(240)],
    );
}

#[test]
fn a_verified_receipt_is_byte_stable_across_repeated_verification() {
    let fixture = Fixture::new();
    let (_, receipt) = minted(&fixture, &fixture_baseline("published floors"));
    let canonical = receipt.encode().expect("receipt JSON");

    let first = BatteryReceipt::verify(&canonical).expect("canonical receipt verifies");
    let again = first.encode().expect("re-encoded receipt");
    let second = BatteryReceipt::verify(&again).expect("a verified receipt verifies again");

    assert_eq!(first, receipt);
    assert_eq!(second, receipt);
    assert_eq!(again, canonical);
    assert_eq!(second.encode().expect("re-encoded receipt"), canonical);
    assert_eq!(
        receipt.digest(),
        Digest::compute(Domain::GateBatteryReceipt, &canonical)
    );
}

#[test]
fn receipt_encoding_reuses_one_byte_identical_canonical_report() {
    let fixture = Fixture::new();
    let (_, receipt) = minted(&fixture, &fixture_baseline("cached canonical report"));

    let first = receipt.run().canonical().expect("first canonical report");
    let second = receipt.run().canonical().expect("reused canonical report");
    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "immutable report bytes must be shared rather than serialized again",
    );
    assert_eq!(
        receipt.encode().expect("optimized receipt encoding"),
        wire::encode(&receipt).expect("derived canonical receipt encoding"),
        "receipt framing must remain byte-identical to the canonical serializer",
    );

    let cloned = receipt.clone();
    let cloned_bytes = cloned.run().canonical().expect("cloned canonical report");
    assert!(
        std::sync::Arc::ptr_eq(&first, &cloned_bytes),
        "receipt clones must retain the same immutable representation",
    );
    assert_eq!(cloned, receipt, "the cache cannot participate in identity");
}

#[test]
#[ignore = "requires durable verifier evidence in the current worktree"]
fn production_receipt_encoding_benchmark() {
    const PAIR_COUNT: usize = 3;
    const MAX_MEDIAN_WALL_RATIO: f64 = 0.75;

    let repository = gix::discover(".").expect("discover the benchmark worktree");
    let path = repository
        .git_dir()
        .join(".spice-rs/full-verifier-evidence.json");
    let stored = fs::read(path).expect("read durable full-verifier evidence");
    assert!(
        stored.starts_with(&[0x1f, 0x8b]),
        "current durable full-verifier evidence must use its gzip container",
    );
    let mut document = Vec::new();
    GzDecoder::new(stored.as_slice())
        .read_to_end(&mut document)
        .expect("expand durable full-verifier evidence");
    let envelope: serde_json::Value = wire::decode(&document).expect("decode verifier evidence");
    let receipt_bytes = envelope["battery"]
        .as_str()
        .expect("verifier evidence carries a battery receipt")
        .as_bytes();
    let receipt = BatteryReceipt::verify(receipt_bytes).expect("admit durable battery receipt");
    let reference = wire::encode(&receipt).expect("reference canonical receipt encoding");
    let optimized = receipt
        .encode()
        .expect("optimized canonical receipt encoding");
    assert_eq!(
        optimized, reference,
        "optimization must preserve exact bytes"
    );

    let measure = |optimized: bool| {
        let started = std::time::Instant::now();
        let bytes = if optimized {
            receipt.encode().expect("optimized receipt encoding")
        } else {
            wire::encode(&receipt).expect("reference receipt encoding")
        };
        std::hint::black_box(bytes);
        started.elapsed().as_nanos()
    };
    let pairs = (0..PAIR_COUNT)
        .map(|ordinal| {
            if ordinal % 2 == 0 {
                (measure(false), measure(true))
            } else {
                let optimized = measure(true);
                (measure(false), optimized)
            }
        })
        .collect::<Vec<_>>();
    let mut ratios = pairs
        .iter()
        .map(|(reference, optimized)| *optimized as f64 / *reference as f64)
        .collect::<Vec<_>>();
    ratios.sort_by(f64::total_cmp);
    let median = ratios[PAIR_COUNT / 2];
    println!(
        "[battery-receipt-benchmark] bytes={} median_wall_ratio={median:.3} pairs={pairs:?}",
        reference.len(),
    );
    assert!(
        median <= MAX_MEDIAN_WALL_RATIO,
        "cached production encoding did not cut median wall time by at least 25%: {pairs:?}",
    );
}

/// A pair this build does not carry is evidence about foreign bytes and nothing
/// more, so verification hands the bytes back exactly as they arrived.
///
/// Dispatch runs before the canonical byte comparison, which is what lets these
/// re-serialized candidates reach the version boundary at all; a build that
/// compared bytes first would refuse every foreign schema as merely
/// noncanonical and never name the pair it observed.
#[test]
fn an_unknown_schema_or_gate_set_is_returned_unread() {
    let fixture = Fixture::new();
    let (_, receipt) = minted(&fixture, &fixture_baseline("published floors"));
    let canonical = receipt.encode().expect("receipt JSON");
    let value: serde_json::Value = serde_json::from_slice(&canonical).expect("receipt JSON value");
    let foreign = Digest::compute(Domain::GateSet, b"a gate set this build never compiled");

    for (name, forged, version, gate_set) in [
        (
            "unknown schema",
            mutate(&value, |candidate| {
                candidate["version"] = serde_json::json!(GATE_BATTERY_RECEIPT_VERSION + 1);
            }),
            GATE_BATTERY_RECEIPT_VERSION + 1,
            receipt.gate_set().clone(),
        ),
        (
            "unknown gate set",
            mutate(&value, |candidate| {
                candidate["run"]["gate_set"] = serde_json::json!(foreign.to_string());
            }),
            GATE_BATTERY_RECEIPT_VERSION,
            foreign.clone(),
        ),
    ] {
        let bytes = serde_json::to_vec(&forged).expect("candidate JSON");
        let refusal = BatteryReceipt::verify(&bytes).expect_err("a foreign pair must be refused");
        let unsupported = refusal.unsupported().unwrap_or_else(|| {
            panic!("{name} must refuse at the version boundary, observed {refusal}")
        });
        assert_eq!(unsupported.version(), version, "{name}");
        assert_eq!(unsupported.gate_set(), &gate_set, "{name}");
        assert_eq!(unsupported.bytes(), bytes, "{name} must retain its bytes");
        assert_eq!(
            unsupported.digest(),
            &Digest::compute(Domain::GateBatteryReceipt, &bytes),
            "{name} identity must be the digest of the bytes that arrived",
        );
        let rendered = refusal.to_string();
        assert!(
            !rendered.contains(std::str::from_utf8(&bytes).expect("candidate is UTF-8")),
            "{name} refusal must not spell foreign bytes into a current message: {rendered}",
        );
        for (schema, digest) in supported_pairs() {
            assert!(
                rendered.contains(&format!("schema {schema} with gate set {digest}")),
                "{name} refusal must name every pair this build interprets: {rendered}",
            );
        }
    }
}

/// Every invariant is checked before the bytes are compared, which is what lets
/// each forged candidate above name the invariant it violates. That ordering
/// leaves the byte comparison itself as the only refusal no forgery reaches, so
/// it is proven here directly: a receipt that is semantically perfect but
/// reserialized in another key order must still be refused.
#[test]
fn a_semantically_valid_receipt_in_another_key_order_is_still_refused() {
    let fixture = Fixture::new();
    let (_, receipt) = minted(&fixture, &fixture_baseline("published floors"));
    let canonical = receipt.encode().expect("receipt JSON");
    BatteryReceipt::verify(&canonical).expect("canonical bytes verify");

    // serde_json::Map is a BTreeMap here, so a Value round trip sorts the
    // receipt's fields alphabetically instead of the order the struct declares.
    let value: serde_json::Value = serde_json::from_slice(&canonical).expect("receipt JSON value");
    let reordered = serde_json::to_vec(&value).expect("candidate JSON");
    assert_ne!(
        reordered, canonical,
        "a Value round trip must actually reorder the receipt, or this proves nothing"
    );

    let refusal = BatteryReceipt::verify(&reordered).expect_err("reordered bytes must be refused");
    assert!(refusal.unsupported().is_none());
    assert_eq!(
        refusal.to_string(),
        "battery receipt bytes are not canonical"
    );
}

/// The report's own gate-set invariant is deliberately absent from this matrix:
/// a receipt naming a foreign gate set never reaches it, because dispatch
/// refuses the pair first. That door is proven by
/// `an_unknown_schema_or_gate_set_is_returned_unread` and, for bare report
/// bytes, by `partial_and_reordered_vectors_cannot_decode_as_complete`.
#[test]
fn truncated_tampered_and_reordered_receipts_are_refused_by_the_invariant_they_break() {
    let fixture = Fixture::new();
    let (_, receipt) = minted(&fixture, &fixture_baseline("published floors"));
    let canonical = receipt.encode().expect("receipt JSON");
    let value: serde_json::Value = serde_json::from_slice(&canonical).expect("receipt JSON value");

    // Truncated bytes cannot name a pair at all, so they never reach dispatch.
    let truncated = &canonical[..canonical.len() / 2];
    let refusal = BatteryReceipt::verify(truncated).expect_err("truncated bytes must be refused");
    assert!(refusal.unsupported().is_none());
    assert!(
        refusal
            .to_string()
            .contains("battery receipt has no readable version envelope"),
        "truncation must be named as an unreadable envelope, observed {refusal}",
    );

    let incomplete = format!(
        "complete battery report requires {} results, received {}",
        GateId::BATTERY.len(),
        GateId::BATTERY.len() - 1
    );
    let candidates = vec![
        (
            "battery receipt names a foreign canonical manifest digest",
            mutate(&value, |candidate| {
                candidate["manifest"] = serde_json::json!(
                    Digest::compute(Domain::GateBatteryManifest, b"a manifest never checked in")
                        .to_string()
                );
            }),
        ),
        (
            "battery receipt report digest disagrees with its own run",
            mutate(&value, |candidate| {
                candidate["report"] = serde_json::json!(
                    Digest::compute(Domain::GateBatteryReport, b"a report never run").to_string()
                );
            }),
        ),
        (
            incomplete.as_str(),
            mutate(&value, |candidate| {
                candidate["run"]["results"]
                    .as_array_mut()
                    .expect("result vector")
                    .pop();
            }),
        ),
        (
            "battery result 0 is missing or reordered",
            mutate(&value, |candidate| {
                candidate["run"]["results"]
                    .as_array_mut()
                    .expect("result vector")
                    .swap(0, 1);
            }),
        ),
        (
            "arm state or reason disagrees with its retained family evidence",
            mutate(&value, |candidate| {
                candidate["run"]["results"][0]["reason"] = serde_json::json!("forged reason");
            }),
        ),
    ];
    for (invariant, forged) in candidates {
        let bytes = serde_json::to_vec(&forged).expect("candidate JSON");
        let refusal = BatteryReceipt::verify(&bytes).expect_err("a forged receipt must be refused");
        assert!(
            refusal.to_string().contains(invariant),
            "expected a refusal naming {invariant:?}, observed {refusal}",
        );
    }
}

/// A receipt proves internal agreement and current contract identity. It is not
/// a signature, and nothing cross-checks the caller-supplied baseline, so the
/// honest claim about a changed one is that it is a different receipt.
#[test]
fn a_receipt_bound_to_another_baseline_is_a_different_receipt() {
    let fixture = Fixture::new();
    let published = fixture_baseline("published floors");
    let (report, receipt) = minted(&fixture, &published);
    let successor = BatteryReceipt::mint(report, &fixture_baseline("tightened floors"));

    let bytes = successor.encode().expect("successor JSON");
    let verified = BatteryReceipt::verify(&bytes).expect("a rebaselined receipt is still valid");

    assert_eq!(verified, successor);
    assert_eq!(verified.results(), receipt.results());
    assert_eq!(verified.report(), receipt.report());
    assert_ne!(verified.baseline(), receipt.baseline());
    assert_ne!(
        verified.digest(),
        receipt.digest(),
        "a receipt bound to another baseline must not share the published one's identity",
    );
}

#[test]
fn an_interrupted_publication_leaves_the_previous_receipt_intact() {
    let fixture = Fixture::new();
    let (report, receipt) = minted(&fixture, &fixture_baseline("published floors"));
    let directory = fixture.root.join("receipts");
    fs::create_dir_all(&directory).expect("receipt directory");
    let destination = directory.join("battery-receipt.json");

    receipt.publish(&destination).expect("publish the receipt");
    let published = fs::read(&destination).expect("published receipt");
    assert_eq!(published, receipt.encode().expect("receipt JSON"));
    BatteryReceipt::verify(&published).expect("a published receipt verifies from disk");

    // A successor whose bytes are completely staged but never swapped in: the
    // exact shape of a run that ended between writing and publishing.
    let successor = BatteryReceipt::mint(report, &fixture_baseline("tightened floors"));
    let staging = atomic::Destination::new(&destination).expect("resolve destination");
    let mut staged = staging.stage().expect("stage the successor");
    staged
        .write_all(&successor.encode().expect("successor JSON"))
        .expect("write the staged bytes");
    drop(staged);

    assert_eq!(
        fs::read(&destination).expect("published receipt"),
        published,
        "an interrupted publication must leave the previous receipt exactly as it was",
    );
    let mut remaining: Vec<_> = fs::read_dir(&directory)
        .expect("read the receipt directory")
        .map(|entry| entry.expect("directory entry").file_name())
        .collect();
    remaining.sort_unstable();
    assert_eq!(
        remaining,
        [destination.file_name().expect("receipt file name")],
        "an interrupted publication must leave no staged bytes behind",
    );
}
