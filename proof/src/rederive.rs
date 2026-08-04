//! Mutation-free re-derivation of one existing detached proof.

use std::error::Error;
use std::fmt;
use std::path::Path;

use wire::ProofEvidence;
use wire::manifest::{ProofStanding, ReleaseTarget};

use crate::config::Config;
use crate::container::Limits;
use crate::environment::{EnvironmentError, SealedEnvironment};
use crate::evidence::{AdmittedReceipt, EvidenceError, admit_receipt, verify_admitted};
use crate::layout::{InputError, Layout, LayoutError};
use crate::payload::{AdmittedPayload, PayloadError, admit};
use crate::replay::{ReplayError, ReplayVerdict, replay};
use crate::seal::ReplayRefusal;

/// Re-execute the current battery and verify one existing detached receipt.
///
/// Payload and receipt bytes are admitted before environment setup. The exact
/// target-matched environment then executes the existing current replay path
/// once. Success re-derives the shared evidence DTO without publishing or
/// changing any repository or appliance state.
///
/// # Errors
///
/// Returns the exact input, admission, layout, environment, execution,
/// standing, or evidence mismatch that prevented re-derivation.
pub fn rederive(
    layout: &Layout,
    config: &Config,
    workspace: impl AsRef<Path>,
    limits: Limits,
) -> Result<ProofEvidence, RederiveError> {
    let payload_bytes = layout.read_payload().map_err(RederiveError::ReadPayload)?;
    let admitted = admit(&payload_bytes).map_err(RederiveError::AdmitPayload)?;
    let receipt = layout
        .receipt_for(admitted.value())
        .map_err(RederiveError::Layout)?;
    let receipt_bytes = receipt.read().map_err(RederiveError::ReadReceipt)?;
    let admitted_receipt = admit_receipt(&receipt_bytes).map_err(RederiveError::AdmitReceipt)?;
    let environment = SealedEnvironment::current(config, &admitted, workspace, limits)
        .map_err(RederiveError::Environment)?;

    finish(&admitted, &admitted_receipt, environment.target(), || {
        replay(&environment)
    })
}

fn finish<R>(
    admitted: &AdmittedPayload,
    receipt: &AdmittedReceipt,
    environment_target: ReleaseTarget,
    run: R,
) -> Result<ProofEvidence, RederiveError>
where
    R: FnOnce() -> Result<ReplayVerdict, ReplayError>,
{
    let payload = admitted.value();
    if payload.target != environment_target {
        return Err(RederiveError::EnvironmentTarget {
            payload: payload.target,
            environment: environment_target,
        });
    }

    let replay = run().map_err(RederiveError::Replay)?;
    if replay.standing() != ProofStanding::Clean {
        return Err(RederiveError::Refused(ReplayRefusal::new(replay)));
    }

    verify_admitted(admitted, receipt, &replay)
        .map(ProofEvidence::from)
        .map_err(RederiveError::Evidence)
}

/// Why existing detached proof evidence could not be re-derived.
#[derive(Debug)]
pub enum RederiveError {
    ReadPayload(InputError),
    AdmitPayload(PayloadError),
    Layout(LayoutError),
    ReadReceipt(InputError),
    AdmitReceipt(EvidenceError),
    Environment(EnvironmentError),
    EnvironmentTarget {
        payload: ReleaseTarget,
        environment: ReleaseTarget,
    },
    Replay(ReplayError),
    Refused(ReplayRefusal),
    Evidence(EvidenceError),
}

impl fmt::Display for RederiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadPayload(source) | Self::ReadReceipt(source) => source.fmt(formatter),
            Self::AdmitPayload(source) => source.fmt(formatter),
            Self::Layout(source) => source.fmt(formatter),
            Self::AdmitReceipt(source) | Self::Evidence(source) => source.fmt(formatter),
            Self::Environment(source) => source.fmt(formatter),
            Self::EnvironmentTarget {
                payload,
                environment,
            } => write!(
                formatter,
                "sealed environment target {environment} does not match payload target {payload}"
            ),
            Self::Replay(source) => source.fmt(formatter),
            Self::Refused(source) => source.fmt(formatter),
        }
    }
}

impl Error for RederiveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadPayload(source) | Self::ReadReceipt(source) => Some(source),
            Self::AdmitPayload(source) => Some(source),
            Self::Layout(source) => Some(source),
            Self::AdmitReceipt(source) | Self::Evidence(source) => Some(source),
            Self::Environment(source) => Some(source),
            Self::Replay(source) => Some(source),
            Self::Refused(source) => Some(source),
            Self::EnvironmentTarget { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use sill::Deadlines;
    use wire::manifest::{
        BuildPins, InventoryEntry, ProofEnvironment, ProofReceipt, ReleaseTarget, TargetPayload,
    };
    use wire::{Digest, Domain};

    use super::{RederiveError, finish};
    use crate::config::Config;
    use crate::container::Limits;
    use crate::evidence::{AdmittedReceipt, EvidenceError, admit_receipt};
    use crate::layout::{Input, InputError, Layout};
    use crate::payload::{AdmittedPayload, admit};
    use crate::replay::{Battery, CheckKind, CheckStanding, ReplayError, ReplayVerdict};

    #[test]
    fn canonical_existing_proof_runs_once_and_returns_the_stored_receipt_identity() {
        let fixture = Fixture::new(ReleaseTarget::X86_64UnknownLinuxMusl);
        let (admitted, receipt, replay) = fixture.prepared();
        let runs = Cell::new(0);

        let evidence = finish(&admitted, &receipt, fixture.target, || {
            runs.set(runs.get() + 1);
            Ok(replay.clone())
        })
        .expect("canonical existing proof re-derives");

        assert_eq!(runs.get(), 1);
        assert_eq!(evidence.target, fixture.target);
        assert_eq!(evidence.payload, *admitted.digest());
        assert_eq!(evidence.receipt, *receipt.digest());
        assert_eq!(evidence.battery, *replay.battery_digest());
        assert_eq!(evidence.replay, *replay.replay_digest());
    }

    #[test]
    fn target_mismatch_infrastructure_and_non_clean_standings_remain_distinct() {
        let fixture = Fixture::new(ReleaseTarget::X86_64UnknownLinuxMusl);
        let (admitted, receipt, replay) = fixture.prepared();
        let ran = Cell::new(false);
        let mismatch = finish(
            &admitted,
            &receipt,
            ReleaseTarget::Aarch64AppleDarwin,
            || {
                ran.set(true);
                Ok(replay)
            },
        )
        .expect_err("environment target mismatch refuses");
        assert!(matches!(mismatch, RederiveError::EnvironmentTarget { .. }));
        assert!(!ran.get());

        let infrastructure = finish(&admitted, &receipt, fixture.target, || {
            Err(ReplayError::fixture(CheckKind::Gate, "runner unavailable"))
        })
        .expect_err("infrastructure failure remains distinct");
        assert!(matches!(infrastructure, RederiveError::Replay(_)));

        for standing in [
            CheckStanding::Failed { code: Some(1) },
            CheckStanding::TimedOut { code: None },
            CheckStanding::OutputTruncated,
            CheckStanding::OutputIncomplete,
        ] {
            let replay = fixture.refused_replay(standing);
            let error = finish(&admitted, &receipt, fixture.target, || Ok(replay))
                .expect_err("non-clean replay refuses");
            let RederiveError::Refused(refusal) = error else {
                panic!("non-clean standing lost its typed refusal")
            };
            assert_eq!(refusal.replay().checks()[0].standing(), standing);
        }
    }

    #[test]
    fn every_stored_identity_drift_reaches_the_existing_typed_verifier() {
        let fixture = Fixture::new(ReleaseTarget::X86_64UnknownLinuxMusl);
        let (admitted, _, replay) = fixture.prepared();
        let receipt = fixture.receipt(&admitted, &replay);
        let other = |domain| Digest::compute(domain, b"drifted identity");

        let mut drifted = receipt.clone();
        drifted.target = ReleaseTarget::Aarch64AppleDarwin;
        assert_evidence_error(&fixture, &admitted, &replay, drifted, |error| {
            matches!(error, EvidenceError::Target { .. })
        });

        let mut drifted = receipt.clone();
        drifted.payload = other(Domain::DistTargetPayload);
        assert_evidence_error(&fixture, &admitted, &replay, drifted, |error| {
            *error == EvidenceError::PayloadDigest
        });

        let mut drifted = receipt.clone();
        drifted.environment =
            ProofEnvironment::oci(format!("registry.example/drift@sha256:{}", "5".repeat(64)))
                .expect("drifted image remains canonical");
        assert_evidence_error(&fixture, &admitted, &replay, drifted, |error| {
            matches!(error, EvidenceError::Environment { .. })
        });

        let mut drifted = receipt.clone();
        drifted.battery = other(Domain::ProofBattery);
        assert_evidence_error(&fixture, &admitted, &replay, drifted, |error| {
            *error == EvidenceError::BatteryDigest
        });

        let mut drifted = receipt.clone();
        drifted.replay = other(Domain::ProofReplayV2);
        assert_evidence_error(&fixture, &admitted, &replay, drifted, |error| {
            *error == EvidenceError::ReplayDigest
        });

        let mut drifted = receipt;
        drifted.standing = wire::manifest::ProofStanding::Refused;
        assert_evidence_error(&fixture, &admitted, &replay, drifted, |error| {
            *error == EvidenceError::Standing
        });

        let mut drifted = fixture.prepared().1;
        drifted.replace_digest(other(Domain::ProofReceiptV2));
        let error = finish(&admitted, &drifted, fixture.target, || Ok(replay))
            .expect_err("stored receipt digest drift refuses");
        assert!(matches!(
            error,
            RederiveError::Evidence(EvidenceError::ReceiptDigest)
        ));
    }

    #[test]
    fn absent_and_noncanonical_receipts_refuse_before_environment_setup() {
        let fixture = Fixture::new(ReleaseTarget::X86_64UnknownLinuxMusl);
        let layout = fixture.layout();
        let config = Config::fixture(None, None);
        let before = snapshot(&fixture.selected);

        let missing = super::rederive(&layout, &config, &fixture.worktree, limits())
            .expect_err("missing receipt refuses");
        assert!(matches!(
            missing,
            RederiveError::ReadReceipt(InputError::Missing {
                input: Input::Receipt,
                ..
            })
        ));
        assert_eq!(snapshot(&fixture.selected), before);

        let (admitted, _, replay) = fixture.prepared();
        let receipt = layout
            .receipt_for(admitted.value())
            .expect("fixture receipt path");
        fs::create_dir_all(receipt.path().parent().expect("receipt parent"))
            .expect("create receipt parent");
        let mut noncanonical =
            wire::encode(&fixture.receipt(&admitted, &replay)).expect("fixture receipt encodes");
        noncanonical.push(b'\n');
        fs::write(receipt.path(), noncanonical).expect("write noncanonical receipt");
        let with_noncanonical_receipt = snapshot(&fixture.selected);

        let refused = super::rederive(&layout, &config, &fixture.worktree, limits())
            .expect_err("noncanonical receipt refuses");
        assert!(matches!(
            refused,
            RederiveError::AdmitReceipt(EvidenceError::NonCanonicalReceipt)
        ));
        assert_eq!(snapshot(&fixture.selected), with_noncanonical_receipt);
        assert_ne!(before, with_noncanonical_receipt);
    }

    #[cfg(all(target_arch = "aarch64", target_vendor = "apple", target_os = "macos"))]
    #[test]
    fn public_replay_executes_the_current_battery_once_and_changes_no_selected_byte() {
        let fixture = Fixture::new(ReleaseTarget::Aarch64AppleDarwin);
        let runner = fixture.root.join("runner");
        let invocations = fixture.root.join("invocations");
        let script = format!(
            concat!(
                "#!/bin/sh\n",
                "[ \"$1\" = spice-proof-native-runner-v1 ] || exit 71\n",
                "identity=$2\n",
                "challenge=$3\n",
                "marker=spice-proof-native-attestation-v1\n",
                "claims=repository=read-only:\n",
                "claims=${{claims}}cargo-home=fresh-empty:\n",
                "claims=${{claims}}cargo-target=fresh-empty:\n",
                "claims=${{claims}}network=loopback-only:\n",
                "claims=${{claims}}host-dependencies=denied\n",
                "printf '%s:runner=%s:challenge=%s:%s\\n' ",
                "\"$marker\" \"$identity\" \"$challenge\" \"$claims\" >&2\n",
                "printf 'ran\\n' >> '{}'\n",
                "exit 0\n"
            ),
            invocations.display()
        );
        {
            let _write_guard = crate::test_support::executable_write_guard();
            fs::write(&runner, script).expect("write native runner");
            make_executable(&runner);
        }
        let config = Config::fixture(None, Some(runner));

        let sealed = crate::seal(&fixture.layout(), &config, &fixture.worktree, limits())
            .expect("fixture proof seals");
        fixture.plant_state_markers();
        fs::write(&invocations, b"").expect("reset invocation evidence");
        let before = snapshot(&fixture.selected);

        let rederived = super::rederive(&fixture.layout(), &config, &fixture.worktree, limits())
            .expect("existing proof re-derives");

        assert_eq!(rederived, sealed);
        assert_eq!(snapshot(&fixture.selected), before);
        assert_eq!(
            fs::read_to_string(invocations)
                .expect("read invocation evidence")
                .lines()
                .count(),
            CheckKind::ALL.len(),
            "one execution of every current battery member"
        );
        assert_eq!(
            crate::compiled_target().expect("compiled host is supported"),
            ReleaseTarget::Aarch64AppleDarwin
        );
    }

    fn assert_evidence_error(
        fixture: &Fixture,
        admitted: &AdmittedPayload,
        replay: &ReplayVerdict,
        receipt: ProofReceipt,
        matches: impl FnOnce(&EvidenceError) -> bool,
    ) {
        let receipt = admit_receipt(&wire::encode(&receipt).expect("drifted receipt encodes"))
            .expect("drifted receipt remains canonical");
        let error = finish(admitted, &receipt, fixture.target, || Ok(replay.clone()))
            .expect_err("stored identity drift refuses");
        let RederiveError::Evidence(error) = error else {
            panic!("stored identity drift bypassed evidence verification")
        };
        assert!(matches(&error), "unexpected evidence error: {error:?}");
    }

    struct Fixture {
        root: PathBuf,
        selected: PathBuf,
        worktree: PathBuf,
        common: PathBuf,
        target: ReleaseTarget,
        payload: TargetPayload,
        payload_bytes: Vec<u8>,
    }

    impl Fixture {
        fn new(target: ReleaseTarget) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-proof-rederive-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let selected = root.join("selected");
            let worktree = selected.join("worktree");
            let common = selected.join("common");
            fs::create_dir_all(&worktree).expect("fixture worktree");
            fs::create_dir_all(&common).expect("fixture common directory");
            let payload = payload(target);
            let payload_bytes = wire::encode(&payload).expect("fixture payload encodes");
            let layout = Layout::new(&worktree, common.join(".spice-rust/proof"), target);
            fs::create_dir_all(layout.payload_path().parent().expect("payload parent"))
                .expect("create payload parent");
            fs::write(layout.payload_path(), &payload_bytes).expect("write payload");
            Self {
                root,
                selected,
                worktree,
                common,
                target,
                payload,
                payload_bytes,
            }
        }

        fn layout(&self) -> Layout {
            Layout::new(
                &self.worktree,
                self.common.join(".spice-rust/proof"),
                self.target,
            )
        }

        fn prepared(&self) -> (AdmittedPayload, AdmittedReceipt, ReplayVerdict) {
            let admitted = admit(&self.payload_bytes).expect("fixture payload admits");
            let replay = self.clean_replay();
            let receipt = self.receipt(&admitted, &replay);
            let receipt = admit_receipt(&wire::encode(&receipt).expect("fixture receipt encodes"))
                .expect("fixture receipt admits");
            (admitted, receipt, replay)
        }

        fn receipt(&self, admitted: &AdmittedPayload, replay: &ReplayVerdict) -> ProofReceipt {
            replay
                .receipt(self.payload(), admitted.digest().clone())
                .expect("fixture replay matches payload")
        }

        fn payload(&self) -> &TargetPayload {
            &self.payload
        }

        fn clean_replay(&self) -> ReplayVerdict {
            ReplayVerdict::fixture_clean(
                self.target,
                self.environment(),
                Battery::current().digest().clone(),
            )
        }

        fn refused_replay(&self, first: CheckStanding) -> ReplayVerdict {
            let mut standings = [CheckStanding::Clean; CheckKind::ALL.len()];
            standings[0] = first;
            ReplayVerdict::fixture_with(
                self.target,
                self.environment(),
                Battery::current().digest().clone(),
                standings,
            )
        }

        fn environment(&self) -> ProofEnvironment {
            match self.target {
                ReleaseTarget::X86_64UnknownLinuxMusl => {
                    ProofEnvironment::oci(&self.payload.pins.proof_container)
                        .expect("fixture image is pinned")
                }
                ReleaseTarget::Aarch64AppleDarwin => ProofEnvironment::native(Digest::compute(
                    Domain::ProofNativeRunner,
                    b"fixture native runner",
                )),
            }
        }

        #[cfg(all(target_arch = "aarch64", target_vendor = "apple", target_os = "macos"))]
        fn plant_state_markers(&self) {
            for (path, bytes) in [
                ("worktree/src/lib.rs", b"repository bytes".as_slice()),
                (
                    "common/.spice-rust/journal.sqlite",
                    b"journal bytes".as_slice(),
                ),
                ("common/.spice-rust/fold/head", b"fold bytes".as_slice()),
                (
                    "common/.spice-rust/resident.json",
                    b"discovery bytes".as_slice(),
                ),
            ] {
                let path = self.selected.join(path);
                fs::create_dir_all(path.parent().expect("marker parent"))
                    .expect("create marker parent");
                fs::write(path, bytes).expect("write state marker");
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove fixture");
        }
    }

    fn payload(target: ReleaseTarget) -> TargetPayload {
        let pins = BuildPins {
            rust: "rustc 1.97.1".to_owned(),
            wasm_target: "wasm32-unknown-unknown".to_owned(),
            trunk: "trunk 0.21.14".to_owned(),
            cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
            proof_container: format!("registry.example/proof@sha256:{}", "4".repeat(64)),
        };
        let app = Digest::compute(Domain::DistArtifact, b"app");
        let embedded = Digest::compute(Domain::DistArtifact, b"embedded");
        let native = Digest::compute(Domain::DistArtifact, b"native");
        let inventory = vec![
            InventoryEntry {
                path: "app/app.wasm".to_owned(),
                mode: 0o644,
                bytes: 3,
                digest: app.clone(),
            },
            InventoryEntry {
                path: "bin/spice".to_owned(),
                mode: 0o755,
                bytes: 6,
                digest: native.clone(),
            },
            InventoryEntry {
                path: "embed/app.bin".to_owned(),
                mode: 0o644,
                bytes: 8,
                digest: embedded.clone(),
            },
        ];
        TargetPayload {
            version: 1,
            target,
            source: Digest::compute(Domain::DistArtifact, b"source"),
            app,
            embedded_app: embedded,
            native,
            pins_digest: Digest::compute(
                Domain::DistPins,
                &wire::encode(&pins).expect("pins encode"),
            ),
            inventory_digest: Digest::compute(
                Domain::DistInventory,
                &wire::encode(&inventory).expect("inventory encodes"),
            ),
            pins,
            inventory,
        }
    }

    fn limits() -> Limits {
        Limits::new(
            Deadlines::new(
                Duration::from_secs(30),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .expect("fixture deadlines"),
            Duration::from_millis(10),
            4096,
        )
        .expect("fixture limits")
    }

    fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut pending = vec![root.to_path_buf()];
        let mut files = Vec::new();
        while let Some(path) = pending.pop() {
            let mut entries = fs::read_dir(&path)
                .expect("snapshot directory")
                .map(|entry| entry.expect("snapshot entry").path())
                .collect::<Vec<_>>();
            entries.sort();
            for entry in entries.into_iter().rev() {
                if entry.is_dir() {
                    pending.push(entry);
                } else {
                    let relative = entry
                        .strip_prefix(root)
                        .expect("snapshot path belongs to root")
                        .to_path_buf();
                    files.push((relative, fs::read(entry).expect("snapshot file")));
                }
            }
        }
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files
    }

    #[cfg(all(target_arch = "aarch64", target_vendor = "apple", target_os = "macos"))]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("make fixture runner executable");
    }
}
