//! Spelling fixtures for the canonical identity types.
//!
//! `uuid` and `jiff` both accept a family of spellings. WIRE admits exactly
//! one of each, so these fixtures pin the refusals: every spelling a looser
//! parser would silently repair must fail here instead.

use wire::{CanonicalUuid, Digest, Domain, IdentError};

const CANONICAL: &str = "018f82d4-6f59-7d06-92e3-bce9131163c2";

#[test]
fn the_canonical_spelling_is_admitted_and_renders_unchanged() {
    let parsed: CanonicalUuid = CANONICAL.parse().expect("canonical spelling");
    assert_eq!(parsed.to_string(), CANONICAL);
}

#[test]
fn only_the_canonical_uuid_spelling_is_admitted() {
    let refused = [
        ("braces", "{018f82d4-6f59-7d06-92e3-bce9131163c2}"),
        ("upper case", "018F82D4-6F59-7D06-92E3-BCE9131163C2"),
        ("mixed case", "018f82d4-6F59-7d06-92E3-bce9131163c2"),
        ("compact", "018f82d46f597d0692e3bce9131163c2"),
        ("urn", "urn:uuid:018f82d4-6f59-7d06-92e3-bce9131163c2"),
        ("truncated", "018f82d4-6f59-7d06-92e3-bce9131163c"),
        ("overlong", "018f82d4-6f59-7d06-92e3-bce9131163c22"),
        ("shifted separators", "018f82d4-6f597-d06-92e3-bce9131163c2"),
        ("wrong separator", "018f82d4_6f59-7d06-92e3-bce9131163c2"),
        ("non-hex digit", "018f82d4-6f59-7d06-92e3-bce9131163cg"),
        ("leading space", " 018f82d4-6f59-7d06-92e3-bce9131163c2"),
        ("empty", ""),
    ];

    for (spelling, text) in refused {
        let outcome = text.parse::<CanonicalUuid>();
        assert_eq!(
            outcome,
            Err(IdentError::Uuid(text.to_owned())),
            "the {spelling} spelling must be refused, not repaired"
        );
    }
}

#[test]
fn a_non_canonical_uuid_is_refused_by_the_decoder_too() {
    // The refusal has to sit in the type, not in a caller-side check, or a
    // decoded frame could smuggle a spelling the parser would have rejected.
    let admitted = wire::decode::<CanonicalUuid>(format!("\"{CANONICAL}\"").as_bytes());
    let refused =
        wire::decode::<CanonicalUuid>(b"\"018F82D4-6F59-7D06-92E3-BCE9131163C2\"".as_slice());

    assert!(admitted.is_ok(), "the canonical spelling decodes");
    assert!(refused.is_err(), "upper-case text is not a canonical uuid");
}

/// Where each class sits in the sweep below.
///
/// The sweep is a hand-written array, and on its own an array goes stale in
/// silence. The outer match is exhaustive, so a new variant must first select
/// one bounded section; the section tables then pin its exact index.
const fn position(domain: Domain) -> usize {
    match domain {
        Domain::BoardDocument
        | Domain::BoardTaskIdentity
        | Domain::BoardBarrier
        | Domain::ConfigLocal
        | Domain::ConfigPlan
        | Domain::JournalBackupManifest
        | Domain::JournalBackupMember
        | Domain::JournalRestorePlan
        | Domain::DistTargetPayload
        | Domain::DistArtifact
        | Domain::InstallArtifact
        | Domain::InstallPlan
        | Domain::DistPins
        | Domain::DistInventory
        | Domain::DistSourceCommit => early_position(domain),
        Domain::DistReleaseManifest
        | Domain::DistReleaseNotes
        | Domain::DistReleaseNotesContent
        | Domain::DistReleasePackage
        | Domain::DistReleasePlan
        | Domain::ProofBattery
        | Domain::ProofReplayV3
        | Domain::ProofReceiptV3
        | Domain::ProofNativeRunner
        | Domain::ProofNativeClosureFile
        | Domain::ProofNativeClosure
        | Domain::GateFile
        | Domain::GateTree
        | Domain::GateSet
        | Domain::GatePolicy
        | Domain::GateBounds
        | Domain::GateBaseline => middle_position(domain),
        Domain::GateTool
        | Domain::GateInventory
        | Domain::GateInputManifest
        | Domain::GateActionSpec
        | Domain::GateActionKey
        | Domain::GateMeasurement
        | Domain::GateReceipt
        | Domain::GateStateLedger
        | Domain::GateStateReceipt
        | Domain::ConfigExecutableSurface
        | Domain::TaskArtifactPayload
        | Domain::TaskArtifactStore
        | Domain::GateBatteryManifest
        | Domain::GateBatteryReport => late_position(domain),
        Domain::GateFocusedReport
        | Domain::GateRepositoryAuthority
        | Domain::GateSelection
        | Domain::GatePassReceipt
        | Domain::GateAdmission
        | Domain::GateEvidenceLease
        | Domain::GateBatteryReceipt
        | Domain::GateConfiguration
        | Domain::TasteProse
        | Domain::TasteBag
        | Domain::MediaContent
        | Domain::QuoteText
        | Domain::ReportCommandRecord
        | Domain::CliExport
        | Domain::TaskText
        | Domain::RoomSource
        | Domain::PrincipalCapability
        | Domain::RoomProfileEvidence
        | Domain::RoomQuestionItem
        | Domain::InstallPlanRetained => final_position(domain),
    }
}

const fn early_position(domain: Domain) -> usize {
    match domain {
        Domain::BoardDocument => 0,
        Domain::BoardTaskIdentity => 1,
        Domain::BoardBarrier => 2,
        Domain::ConfigLocal => 3,
        Domain::ConfigPlan => 4,
        Domain::JournalBackupManifest => 5,
        Domain::JournalBackupMember => 6,
        Domain::JournalRestorePlan => 7,
        Domain::DistTargetPayload => 8,
        Domain::DistArtifact => 9,
        Domain::InstallArtifact => 10,
        Domain::InstallPlan => 11,
        Domain::DistPins => 12,
        Domain::DistInventory => 13,
        Domain::DistSourceCommit => 14,
        _ => panic!("domain position partition is closed"),
    }
}

const fn middle_position(domain: Domain) -> usize {
    match domain {
        Domain::DistReleaseManifest => 15,
        Domain::DistReleaseNotes => 16,
        Domain::DistReleaseNotesContent => 17,
        Domain::DistReleasePackage => 18,
        Domain::DistReleasePlan => 19,
        Domain::ProofBattery => 20,
        Domain::ProofReplayV3 => 21,
        Domain::ProofReceiptV3 => 22,
        Domain::ProofNativeRunner => 23,
        Domain::ProofNativeClosureFile => 24,
        Domain::ProofNativeClosure => 25,
        Domain::GateFile => 26,
        Domain::GateTree => 27,
        Domain::GateSet => 28,
        Domain::GatePolicy => 29,
        Domain::GateBounds => 30,
        Domain::GateBaseline => 31,
        _ => panic!("domain position partition is closed"),
    }
}

const fn late_position(domain: Domain) -> usize {
    match domain {
        Domain::GateTool => 32,
        Domain::GateInventory => 33,
        Domain::GateInputManifest => 34,
        Domain::GateActionSpec => 35,
        Domain::GateActionKey => 36,
        Domain::GateMeasurement => 37,
        Domain::GateReceipt => 38,
        Domain::GateStateLedger => 39,
        Domain::GateStateReceipt => 40,
        Domain::ConfigExecutableSurface => 41,
        Domain::TaskArtifactPayload => 42,
        Domain::TaskArtifactStore => 43,
        Domain::GateBatteryManifest => 44,
        Domain::GateBatteryReport => 45,
        _ => panic!("domain position partition is closed"),
    }
}

const fn final_position(domain: Domain) -> usize {
    match domain {
        Domain::GateFocusedReport => 46,
        Domain::GateBatteryReceipt => 47,
        Domain::GateConfiguration => 48,
        Domain::GateRepositoryAuthority => 49,
        Domain::GateSelection => 50,
        Domain::GatePassReceipt => 51,
        Domain::GateAdmission => 52,
        Domain::GateEvidenceLease => 53,
        Domain::TasteProse => 54,
        Domain::TasteBag => 55,
        Domain::MediaContent => 56,
        Domain::QuoteText => 57,
        Domain::ReportCommandRecord => 58,
        Domain::CliExport => 59,
        Domain::TaskText => 60,
        Domain::RoomSource => 61,
        Domain::PrincipalCapability => 62,
        Domain::RoomProfileEvidence => 63,
        Domain::RoomQuestionItem => 64,
        Domain::InstallPlanRetained => 65,
        _ => panic!("domain position partition is closed"),
    }
}

#[test]
fn the_sweep_names_every_domain_exactly_once() {
    for (index, domain) in DOMAINS.iter().enumerate() {
        assert_eq!(
            position(*domain),
            index,
            "the domain sweep and the class list disagree about {domain:?}"
        );
    }
}

/// Every domain, so a new class cannot be added without being swept here.
const DOMAINS: [Domain; 66] = [
    Domain::BoardDocument,
    Domain::BoardTaskIdentity,
    Domain::BoardBarrier,
    Domain::ConfigLocal,
    Domain::ConfigPlan,
    Domain::JournalBackupManifest,
    Domain::JournalBackupMember,
    Domain::JournalRestorePlan,
    Domain::DistTargetPayload,
    Domain::DistArtifact,
    Domain::InstallArtifact,
    Domain::InstallPlan,
    Domain::DistPins,
    Domain::DistInventory,
    Domain::DistSourceCommit,
    Domain::DistReleaseManifest,
    Domain::DistReleaseNotes,
    Domain::DistReleaseNotesContent,
    Domain::DistReleasePackage,
    Domain::DistReleasePlan,
    Domain::ProofBattery,
    Domain::ProofReplayV3,
    Domain::ProofReceiptV3,
    Domain::ProofNativeRunner,
    Domain::ProofNativeClosureFile,
    Domain::ProofNativeClosure,
    Domain::GateFile,
    Domain::GateTree,
    Domain::GateSet,
    Domain::GatePolicy,
    Domain::GateBounds,
    Domain::GateBaseline,
    Domain::GateTool,
    Domain::GateInventory,
    Domain::GateInputManifest,
    Domain::GateActionSpec,
    Domain::GateActionKey,
    Domain::GateMeasurement,
    Domain::GateReceipt,
    Domain::GateStateLedger,
    Domain::GateStateReceipt,
    Domain::ConfigExecutableSurface,
    Domain::TaskArtifactPayload,
    Domain::TaskArtifactStore,
    Domain::GateBatteryManifest,
    Domain::GateBatteryReport,
    Domain::GateFocusedReport,
    Domain::GateBatteryReceipt,
    Domain::GateConfiguration,
    Domain::GateRepositoryAuthority,
    Domain::GateSelection,
    Domain::GatePassReceipt,
    Domain::GateAdmission,
    Domain::GateEvidenceLease,
    Domain::TasteProse,
    Domain::TasteBag,
    Domain::MediaContent,
    Domain::QuoteText,
    Domain::ReportCommandRecord,
    Domain::CliExport,
    Domain::TaskText,
    Domain::RoomSource,
    Domain::PrincipalCapability,
    Domain::RoomProfileEvidence,
    Domain::RoomQuestionItem,
    Domain::InstallPlanRetained,
];

#[test]
fn every_class_leads_with_the_plane_its_tag_names() {
    for domain in DOMAINS {
        let plane = domain
            .tag()
            .split('.')
            .nth(1)
            .expect("a tag names the plane its class belongs to");
        let spelled = format!("{domain:?}");

        assert!(
            spelled.len() > plane.len() && spelled[..plane.len()].eq_ignore_ascii_case(plane),
            "{spelled} is tagged {} but does not lead with the plane {plane}",
            domain.tag()
        );
        assert!(
            spelled[plane.len()..].starts_with(|letter: char| letter.is_ascii_uppercase()),
            "{spelled} must begin a new word after the plane {plane}"
        );
    }
}

#[test]
fn the_same_bytes_under_two_domains_never_collide() {
    for bytes in [b"".as_slice(), b"\x00", b"payload", b"\xff\xfe\x00\x01"] {
        let computed: Vec<Digest> = DOMAINS
            .iter()
            .map(|domain| Digest::compute(*domain, bytes))
            .collect();

        for (index, digest) in computed.iter().enumerate() {
            for other in &computed[index + 1..] {
                assert_ne!(
                    digest, other,
                    "the same bytes under two domains produced one identity"
                );
            }
        }
    }
}

#[test]
fn a_domain_tag_is_frozen_and_distinct_from_every_other() {
    // The tags are the durable half of the identity: changing one silently
    // re-points every digest already published under it.
    let tags: Vec<&str> = DOMAINS.iter().map(|domain| domain.tag()).collect();
    assert_eq!(
        tags,
        [
            "spice.board.document.v1",
            "spice.board.task-identity.v1",
            "spice.board.barrier.v1",
            "spice.config.local.v1",
            "spice.config.plan.v1",
            "spice.journal.backup-manifest.v1",
            "spice.journal.backup-member.v1",
            "spice.journal.restore-plan.v1",
            "spice.dist.target-payload.v1",
            "spice.dist.artifact.v1",
            "spice.install.artifact.v1",
            "spice.install.plan.v1",
            "spice.dist.pins.v1",
            "spice.dist.inventory.v1",
            "spice.dist.source-commit.v1",
            "spice.dist.release-manifest.v1",
            "spice.dist.release-notes.v1",
            "spice.dist.release-notes-content.v1",
            "spice.dist.release-package.v1",
            "spice.dist.release-plan.v1",
            "spice.proof.battery.v1",
            "spice.proof.replay.v3",
            "spice.proof.receipt.v3",
            "spice.proof.native-runner.v1",
            "spice.proof.native-closure-file.v1",
            "spice.proof.native-closure.v1",
            "spice.gate.file.v1",
            "spice.gate.tree.v1",
            "spice.gate.set.v1",
            "spice.gate.policy.v1",
            "spice.gate.bounds.v1",
            "spice.gate.baseline.v1",
            "spice.gate.tool.v1",
            "spice.gate.inventory.v1",
            "spice.gate.input-manifest.v1",
            "spice.gate.action-spec.v1",
            "spice.gate.action-key.v1",
            "spice.gate.measurement.v1",
            "spice.gate.receipt.v1",
            "spice.gate.state-ledger.v1",
            "spice.gate.state-receipt.v1",
            "spice.config.executable-surface.v1",
            "spice.task.artifact-payload.v1",
            "spice.task.artifact-store.v1",
            "spice.gate.battery-manifest.v1",
            "spice.gate.battery-report.v1",
            "spice.gate.focused-report.v1",
            "spice.gate.battery-receipt.v1",
            "spice.gate.configuration.v1",
            "spice.gate.repository-authority.v1",
            "spice.gate.selection.v1",
            "spice.gate.pass-receipt.v1",
            "spice.gate.admission.v1",
            "spice.gate.evidence-lease.v1",
            "spice.taste.prose.v1",
            "spice.taste.bag.v1",
            "spice.media.content.v1",
            "spice.quote.text.v1",
            "spice.report.command-record.v1",
            "spice.cli.export.v1",
            "spice.task.text.v1",
            "spice.room.source.v1",
            "spice.principal.capability.v1",
            "spice.room.profile-evidence.v1",
            "spice.room.question-item.v1",
            "spice.install.coexistence-plan.v1",
        ],
        "a domain tag changed; that re-points digests already published under it"
    );

    for (index, tag) in tags.iter().enumerate() {
        assert!(
            !tag.contains('\0'),
            "{tag} contains a NUL, so the domain framing is no longer injective"
        );
        for other in &tags[index + 1..] {
            assert_ne!(tag, other, "two domains share one tag");
        }
    }
}

#[test]
fn a_computed_digest_is_already_the_one_admitted_spelling() {
    // compute and FromStr share one predicate; this proves they agree rather
    // than assuming it, so a mint can never produce what a decode would refuse.
    for domain in DOMAINS {
        let computed = Digest::compute(domain, b"payload");
        let reparsed: Digest = computed
            .as_str()
            .parse()
            .expect("a computed digest is a canonical spelling");

        assert_eq!(computed, reparsed, "minting and decoding disagree");
        assert_eq!(computed.as_str().len(), 64);
        assert!(
            computed
                .as_str()
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "a computed digest must render lower-case hex"
        );
    }
}

#[test]
fn the_domain_framing_separates_the_tag_from_the_bytes() {
    // Without the separator, tag+bytes concatenation would let one domain's
    // long tag and another's leading bytes hash identically. Stating the exact
    // framing here pins the construction against a silent change.
    let mut expected = blake3::Hasher::new();
    expected.update(Domain::DistArtifact.tag().as_bytes());
    expected.update(&[0]);
    expected.update(b"payload");

    assert_eq!(
        Digest::compute(Domain::DistArtifact, b"payload").as_str(),
        expected.finalize().to_hex().as_str(),
        "the digest is BLAKE3 over tag, NUL, then bytes"
    );
}

#[test]
fn large_executable_inputs_keep_the_canonical_domain_framing() {
    let bytes = vec![0x5a; 4 * 1024 * 1024];
    for domain in [Domain::InstallArtifact, Domain::GateTool] {
        let mut expected = blake3::Hasher::new();
        expected.update(domain.tag().as_bytes());
        expected.update(&[0]);
        expected.update(&bytes);

        assert_eq!(
            Digest::compute(domain, &bytes).as_str(),
            expected.finalize().to_hex().as_str(),
        );
    }
}

#[test]
fn only_the_canonical_digest_spelling_is_admitted() {
    let canonical = Digest::compute(Domain::DistPins, b"pins")
        .as_str()
        .to_owned();
    let upper = canonical.to_ascii_uppercase();

    for spelling in [
        "",
        "abc",
        upper.as_str(),
        &canonical[..63],
        &format!("{canonical}0"),
        &format!("sha256:{canonical}"),
        &canonical.replace('a', "g"),
    ] {
        assert_eq!(
            spelling.parse::<Digest>(),
            Err(IdentError::Digest(spelling.to_owned())),
            "the {spelling:?} spelling must be refused, not repaired"
        );
    }

    assert!(
        canonical.parse::<Digest>().is_ok(),
        "the canonical spelling is admitted"
    );
}

#[test]
fn a_non_canonical_digest_is_refused_by_the_decoder_too() {
    // Same reason as the uuid decoder fixture: a caller-side check would let a
    // decoded frame carry a spelling the parser rejects.
    let canonical = Digest::compute(Domain::DistInventory, b"inventory")
        .as_str()
        .to_owned();

    let admitted = wire::decode::<Digest>(format!("\"{canonical}\"").as_bytes());
    let refused =
        wire::decode::<Digest>(format!("\"{}\"", canonical.to_ascii_uppercase()).as_bytes());

    assert!(admitted.is_ok(), "the canonical spelling decodes");
    assert!(refused.is_err(), "upper-case hex is not a canonical digest");
}
