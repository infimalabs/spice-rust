//! One proof operation from the exact staged payload to detached evidence.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use wire::manifest::{ProofReceipt, ProofStanding, ReleaseTarget};
use wire::{Digest, ProofEvidence};

use crate::config::Config;
use crate::container::Limits;
use crate::environment::{EnvironmentError, SealedEnvironment};
use crate::evidence::{EvidenceError, EvidencePublishError, publish, verify};
use crate::layout::{InputError, Layout, LayoutError, ReceiptInput};
use crate::payload::{AdmittedPayload, PayloadError, admit};
use crate::replay::{ReceiptError, ReplayError, ReplayVerdict, replay};

/// Prove the one payload selected by `layout` and atomically publish evidence.
///
/// The payload is admitted before environment setup. The sealed environment
/// executes the complete current battery once. Only a clean replay that
/// verifies as one payload, target, environment, battery, replay, and receipt
/// identity chain reaches the exact detached-receipt destination.
///
/// # Errors
///
/// Returns a typed input, admission, layout, environment, replay, evidence,
/// or publication failure. Infrastructure failure and non-clean replay both
/// return before the receipt directory or destination is changed.
pub fn seal(
    layout: &Layout,
    config: &Config,
    workspace: impl AsRef<Path>,
    limits: Limits,
) -> Result<ProofEvidence, SealError> {
    let payload_bytes = layout.read_payload().map_err(SealError::ReadPayload)?;
    let admitted = admit(&payload_bytes).map_err(SealError::AdmitPayload)?;
    let receipt = layout
        .receipt_for(admitted.value())
        .map_err(SealError::Layout)?;
    let environment = SealedEnvironment::current(config, &admitted, workspace, limits)
        .map_err(SealError::Environment)?;

    finish(
        &admitted,
        &receipt,
        environment.target(),
        || replay(&environment),
        publish,
    )
}

fn finish<R, P>(
    admitted: &AdmittedPayload,
    receipt_input: &ReceiptInput,
    environment_target: ReleaseTarget,
    run: R,
    publisher: P,
) -> Result<ProofEvidence, SealError>
where
    R: FnOnce() -> Result<ReplayVerdict, ReplayError>,
    P: FnOnce(&ProofReceipt, &Path) -> Result<Digest, EvidencePublishError>,
{
    let payload = admitted.value();
    if payload.target != environment_target {
        return Err(SealError::EnvironmentTarget {
            payload: payload.target,
            environment: environment_target,
        });
    }

    let replay = run().map_err(SealError::Replay)?;
    if replay.standing() != ProofStanding::Clean {
        return Err(SealError::Refused(ReplayRefusal::new(replay)));
    }

    let receipt = replay
        .receipt(payload, admitted.digest().clone())
        .map_err(SealError::Receipt)?;
    let receipt_bytes =
        wire::encode(&receipt).map_err(|source| SealError::EncodeReceipt(source.to_string()))?;
    let verified = verify(admitted, &receipt_bytes, &replay).map_err(SealError::Evidence)?;

    let destination = receipt_input.path();
    let parent = destination
        .parent()
        .expect("a layout-derived receipt path always has a parent");
    fs::create_dir_all(parent).map_err(|source| SealError::PrepareDestination {
        path: parent.to_path_buf(),
        source,
    })?;
    let published = publisher(&receipt, destination).map_err(SealError::Publish)?;
    if published != *verified.receipt_digest() {
        return Err(SealError::PublishedReceipt {
            verified: verified.receipt_digest().clone(),
            published,
        });
    }

    Ok(verified.into())
}

/// A complete replay that ran but did not establish clean standing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayRefusal {
    replay: ReplayVerdict,
}

impl ReplayRefusal {
    pub(crate) const fn new(replay: ReplayVerdict) -> Self {
        Self { replay }
    }

    /// The complete typed replay, including every member's standing.
    #[must_use]
    pub const fn replay(&self) -> &ReplayVerdict {
        &self.replay
    }
}

impl fmt::Display for ReplayRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("sealed proof replay did not establish clean standing")
    }
}

impl Error for ReplayRefusal {}

/// Why one proof operation did not publish clean detached evidence.
#[derive(Debug)]
pub enum SealError {
    ReadPayload(InputError),
    AdmitPayload(PayloadError),
    Layout(LayoutError),
    Environment(EnvironmentError),
    EnvironmentTarget {
        payload: ReleaseTarget,
        environment: ReleaseTarget,
    },
    Replay(ReplayError),
    Refused(ReplayRefusal),
    Receipt(ReceiptError),
    EncodeReceipt(String),
    Evidence(EvidenceError),
    PrepareDestination {
        path: std::path::PathBuf,
        source: io::Error,
    },
    Publish(EvidencePublishError),
    PublishedReceipt {
        verified: Digest,
        published: Digest,
    },
}

impl fmt::Display for SealError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadPayload(source) => source.fmt(formatter),
            Self::AdmitPayload(source) => source.fmt(formatter),
            Self::Layout(source) => source.fmt(formatter),
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
            Self::Receipt(source) => source.fmt(formatter),
            Self::EncodeReceipt(source) => {
                write!(formatter, "proof receipt encoding failed: {source}")
            }
            Self::Evidence(source) => source.fmt(formatter),
            Self::PrepareDestination { path, source } => write!(
                formatter,
                "proof receipt directory could not be prepared at {}: {source}",
                path.display()
            ),
            Self::Publish(source) => source.fmt(formatter),
            Self::PublishedReceipt {
                verified,
                published,
            } => write!(
                formatter,
                "published proof receipt identity {published} does not match verified identity {verified}"
            ),
        }
    }
}

impl Error for SealError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadPayload(source) => Some(source),
            Self::AdmitPayload(source) => Some(source),
            Self::Layout(source) => Some(source),
            Self::Environment(source) => Some(source),
            Self::Replay(source) => Some(source),
            Self::Refused(source) => Some(source),
            Self::Receipt(source) => Some(source),
            Self::Evidence(source) => Some(source),
            Self::PrepareDestination { source, .. } => Some(source),
            Self::Publish(source) => Some(source),
            Self::EnvironmentTarget { .. }
            | Self::EncodeReceipt(_)
            | Self::PublishedReceipt { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    use wire::manifest::{
        BuildPins, InventoryEntry, ProofEnvironment, ReleaseTarget, TargetPayload,
    };
    use wire::{Digest, Domain};

    use super::{SealError, finish};
    use crate::evidence::{EvidencePublishError, publish};
    use crate::layout::{Layout, ReceiptInput};
    use crate::payload::{AdmittedPayload, admit};
    use crate::replay::{Battery, CheckKind, CheckStanding, ReplayError, ReplayVerdict};

    #[test]
    fn clean_proof_runs_once_and_republishes_identical_receipt_bytes_and_identity() {
        let fixture = Fixture::new(ReleaseTarget::X86_64UnknownLinuxMusl);
        let (admitted, receipt) = fixture.prepared();
        let replay = fixture.clean_replay();
        let runs = Cell::new(0);
        let decoys = fixture.plant_decoys();

        let first = finish(
            &admitted,
            &receipt,
            fixture.target,
            || {
                runs.set(runs.get() + 1);
                Ok(replay.clone())
            },
            publish,
        )
        .expect("clean replay publishes");
        let first_bytes = fs::read(receipt.path()).expect("first receipt bytes");
        let second = finish(
            &admitted,
            &receipt,
            fixture.target,
            || {
                runs.set(runs.get() + 1);
                Ok(replay.clone())
            },
            publish,
        )
        .expect("identical replay republishes");
        let second_bytes = fs::read(receipt.path()).expect("second receipt bytes");

        assert_eq!(runs.get(), 2, "one replay per proof operation");
        assert_eq!(replay.checks().len(), CheckKind::ALL.len());
        assert_eq!(first, second);
        assert_eq!(first_bytes, second_bytes);
        assert_eq!(
            first.receipt,
            Digest::compute(Domain::ProofReceiptV3, &first_bytes)
        );
        assert_eq!(
            fs::read(fixture.layout().payload_path()).expect("payload remains readable"),
            fixture.payload_bytes
        );
        fixture.assert_decoys_unchanged(&decoys);
    }

    #[test]
    fn target_mismatch_refuses_before_replay_or_receipt_write() {
        let fixture = Fixture::new(ReleaseTarget::X86_64UnknownLinuxMusl);
        let (admitted, receipt) = fixture.prepared_with_old_receipt();
        let ran = Cell::new(false);
        let published = Cell::new(false);

        let error = finish(
            &admitted,
            &receipt,
            ReleaseTarget::Aarch64AppleDarwin,
            || {
                ran.set(true);
                Ok(fixture.clean_replay())
            },
            |_, _| {
                published.set(true);
                unreachable!("target mismatch cannot publish")
            },
        )
        .expect_err("target mismatch refuses");

        assert!(matches!(error, SealError::EnvironmentTarget { .. }));
        assert!(!ran.get());
        assert!(!published.get());
        assert_eq!(fs::read(receipt.path()).expect("old receipt"), OLD_RECEIPT);
    }

    #[test]
    fn infrastructure_and_non_clean_replays_preserve_existing_receipt() {
        let fixture = Fixture::new(ReleaseTarget::X86_64UnknownLinuxMusl);
        let (admitted, receipt) = fixture.prepared_with_old_receipt();
        let published = Cell::new(0);

        let infrastructure = finish(
            &admitted,
            &receipt,
            fixture.target,
            || Err(ReplayError::fixture(CheckKind::Gate, "runner interrupted")),
            |_, _| {
                published.set(published.get() + 1);
                unreachable!("infrastructure failure cannot publish")
            },
        )
        .expect_err("infrastructure failure refuses");
        assert!(matches!(infrastructure, SealError::Replay(_)));

        for standing in [
            CheckStanding::Failed { code: Some(1) },
            CheckStanding::TimedOut { code: None },
            CheckStanding::Cancelled { code: Some(9) },
            CheckStanding::OutputTruncated,
            CheckStanding::OutputIncomplete,
        ] {
            let refused = fixture.refused_replay(standing);
            let non_clean = finish(
                &admitted,
                &receipt,
                fixture.target,
                || Ok(refused),
                |_, _| {
                    published.set(published.get() + 1);
                    unreachable!("non-clean replay cannot publish")
                },
            )
            .expect_err("non-clean replay refuses");
            let SealError::Refused(refusal) = non_clean else {
                panic!("expected typed replay refusal")
            };
            assert_eq!(refusal.replay().checks()[0].standing(), standing);
        }

        assert_eq!(published.get(), 0);
        assert_eq!(fs::read(receipt.path()).expect("old receipt"), OLD_RECEIPT);
    }

    #[test]
    fn battery_identity_mismatch_refuses_before_publication() {
        let fixture = Fixture::new(ReleaseTarget::X86_64UnknownLinuxMusl);
        let (admitted, receipt) = fixture.prepared_with_old_receipt();
        let wrong_battery = Digest::compute(Domain::ProofBattery, b"another battery");
        let replay =
            ReplayVerdict::fixture_clean(fixture.target, fixture.environment(), wrong_battery);
        let published = Cell::new(false);

        let error = finish(
            &admitted,
            &receipt,
            fixture.target,
            || Ok(replay),
            |_, _| {
                published.set(true);
                unreachable!("unverified evidence cannot publish")
            },
        )
        .expect_err("wrong battery refuses");

        assert!(matches!(
            error,
            SealError::Evidence(crate::evidence::EvidenceError::BatteryDigest)
        ));
        assert!(!published.get());
        assert_eq!(fs::read(receipt.path()).expect("old receipt"), OLD_RECEIPT);
    }

    #[test]
    fn interrupted_staging_never_replaces_the_existing_receipt() {
        let fixture = Fixture::new(ReleaseTarget::X86_64UnknownLinuxMusl);
        let (admitted, receipt) = fixture.prepared_with_old_receipt();

        let error = finish(
            &admitted,
            &receipt,
            fixture.target,
            || Ok(fixture.clean_replay()),
            |_, destination| {
                let publication = atomic::Destination::new(destination)
                    .expect("fixture receipt parent is present");
                let mut staged = publication.stage().expect("stage interrupted write");
                staged.write_all(b"partial receipt").expect("write partial");
                drop(staged);
                Err(EvidencePublishError::Publish(atomic::Error::Refused(
                    atomic::Refusal::DestinationHasNoFileName {
                        destination: fixture.root.clone(),
                    },
                )))
            },
        )
        .expect_err("interrupted publisher refuses");

        assert!(matches!(error, SealError::Publish(_)));
        assert_eq!(fs::read(receipt.path()).expect("old receipt"), OLD_RECEIPT);
        assert_eq!(
            fs::read_dir(receipt.path().parent().expect("receipt parent"))
                .expect("read receipt parent")
                .count(),
            1,
            "interrupted staging leaves no temporary file"
        );
    }

    const OLD_RECEIPT: &[u8] = b"previous receipt";

    struct Fixture {
        root: std::path::PathBuf,
        worktree: std::path::PathBuf,
        common: std::path::PathBuf,
        target: ReleaseTarget,
        payload: TargetPayload,
        payload_bytes: Vec<u8>,
    }

    impl Fixture {
        fn new(target: ReleaseTarget) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-proof-seal-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let worktree = root.join("worktree");
            let common = root.join("common");
            fs::create_dir_all(&worktree).expect("fixture worktree");
            fs::create_dir_all(&common).expect("fixture common directory");
            let payload = payload(target);
            let payload_bytes = wire::encode(&payload).expect("fixture payload encodes");
            let layout = Layout::new(&worktree, common.join(".spice-rs/proof"), target);
            fs::create_dir_all(layout.payload_path().parent().expect("payload parent"))
                .expect("create payload parent");
            fs::write(layout.payload_path(), &payload_bytes).expect("write payload");
            Self {
                root,
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
                self.common.join(".spice-rs/proof"),
                self.target,
            )
        }

        fn prepared(&self) -> (AdmittedPayload, ReceiptInput) {
            let admitted = admit(&self.payload_bytes).expect("fixture payload admits");
            let receipt = self
                .layout()
                .receipt_for(admitted.value())
                .expect("fixture layout matches");
            (admitted, receipt)
        }

        fn prepared_with_old_receipt(&self) -> (AdmittedPayload, ReceiptInput) {
            let prepared = self.prepared();
            fs::create_dir_all(prepared.1.path().parent().expect("receipt parent exists"))
                .expect("create receipt parent");
            fs::write(prepared.1.path(), OLD_RECEIPT).expect("write old receipt");
            prepared
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
                ReleaseTarget::X86_64PcWindowsMsvc => {
                    unreachable!("this fixture does not construct hosted Windows payloads")
                }
            }
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

        fn plant_decoys(&self) -> Vec<(std::path::PathBuf, Vec<u8>)> {
            ["artifact", "journal", "fold", "resident", "cache"]
                .into_iter()
                .map(|name| {
                    let path = self.root.join(name);
                    let bytes = format!("untouched {name}").into_bytes();
                    fs::write(&path, &bytes).expect("write decoy");
                    (path, bytes)
                })
                .collect()
        }

        fn assert_decoys_unchanged(&self, decoys: &[(std::path::PathBuf, Vec<u8>)]) {
            for (path, expected) in decoys {
                assert_eq!(fs::read(path).expect("read decoy"), expected.as_slice());
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
        let app = Digest::compute(Domain::DistArtifact, b"app generation");
        let native = Digest::compute(Domain::DistArtifact, b"native");
        let mut inventory = vec![InventoryEntry {
            path: target.executable_path().to_owned(),
            mode: 0o755,
            bytes: 6,
            digest: native.clone(),
        }];
        let symbols = crate::test_support::symbols(target, &native, &mut inventory);
        inventory.sort_by(|left, right| left.path.cmp(&right.path));
        TargetPayload {
            version: wire::manifest::TARGET_PAYLOAD_VERSION,
            target,
            source: Digest::compute(Domain::DistArtifact, b"source"),
            app,
            canvas: Digest::compute(Domain::DistArtifact, b"canvas archive"),
            native,
            symbols,
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
}
