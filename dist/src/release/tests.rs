use std::collections::BTreeMap;
use std::convert::Infallible;

use super::*;

const SOURCE: &str = "1111111111111111111111111111111111111111";
const RELEASE: &str = "2222222222222222222222222222222222222222";
const PACKAGE_OBJECT: &str = "3333333333333333333333333333333333333333";

fn digest(domain: Domain, value: &[u8]) -> Digest {
    Digest::compute(domain, value)
}

fn input(action: ReleaseAction) -> PlanInput {
    let version = ProductVersion::parse("1.2.3").expect("fixture version");
    let source_version = ProductVersion::parse("1.2.2").expect("source version");
    let recovering = action == ReleaseAction::TagAndAnnounce;
    let package = digest(Domain::DistReleasePackage, b"package");
    let registry = (action == ReleaseAction::TagAndAnnounce).then(|| RegistryEvidence {
        version: version.clone(),
        release_commit: RELEASE.to_owned(),
        package: package.clone(),
    });
    PlanInput {
        repository: digest(Domain::DistSourceCommit, b"repository"),
        action,
        requested_version: version.clone(),
        source_commit: SOURCE.to_owned(),
        release_commit: RELEASE.to_owned(),
        notes: b"## Highlights\n\n- Exact curated prose.\n".to_vec(),
        manifest: digest(Domain::DistReleaseManifest, b"manifest"),
        package,
        package_object: PACKAGE_OBJECT.to_owned(),
        installed_artifact: digest(Domain::DistArtifact, b"native"),
        targets: vec![
            SealedTarget {
                target: "aarch64-apple-darwin".to_owned(),
                artifact: digest(Domain::DistArtifact, b"mac native"),
                receipt: digest(Domain::ProofReceiptV3, b"mac proof"),
            },
            SealedTarget {
                target: "x86_64-unknown-linux-musl".to_owned(),
                artifact: digest(Domain::DistArtifact, b"linux native"),
                receipt: digest(Domain::ProofReceiptV3, b"linux proof"),
            },
        ],
        observations: ReleaseObservations {
            head: if recovering { RELEASE } else { SOURCE }.to_owned(),
            clean: true,
            bump_claimed: recovering,
            unrecorded_local_commits: 0,
            source_version: source_version.clone(),
            release_version: if recovering { version } else { source_version },
            credentials_valid: true,
            source_ref: None,
            package_ref: None,
            tag_ref: None,
            announcement_ref: None,
            registry,
        },
    }
}

#[derive(Default)]
struct MemoryJournal {
    accepted: BTreeMap<Operation, AcceptedOperation>,
    receipts: BTreeMap<Operation, OperationReceipt>,
}

impl ReleaseJournal for MemoryJournal {
    type Error = Infallible;

    fn accepted(
        &mut self,
        _: &Digest,
        operation: Operation,
    ) -> Result<Option<AcceptedOperation>, Self::Error> {
        Ok(self.accepted.get(&operation).cloned())
    }

    fn receipt(
        &mut self,
        _: &Digest,
        operation: Operation,
    ) -> Result<Option<OperationReceipt>, Self::Error> {
        Ok(self.receipts.get(&operation).cloned())
    }

    fn accept(&mut self, accepted: &AcceptedOperation) -> Result<(), Self::Error> {
        self.accepted.insert(accepted.operation, accepted.clone());
        Ok(())
    }

    fn complete(&mut self, receipt: &OperationReceipt) -> Result<(), Self::Error> {
        self.receipts.insert(receipt.operation, receipt.clone());
        Ok(())
    }
}

struct FakeEffects {
    observations: ReleaseObservations,
    converged: BTreeMap<Operation, OperationState>,
    calls: Vec<Operation>,
    interrupt_before: Option<Operation>,
    interrupt_after: Option<Operation>,
}

impl FakeEffects {
    fn new(plan: &ReleasePlan) -> Self {
        Self {
            observations: plan.observations.clone(),
            converged: BTreeMap::new(),
            calls: Vec::new(),
            interrupt_before: None,
            interrupt_after: None,
        }
    }

    fn result(plan: &ReleasePlan, operation: Operation) -> OperationState {
        match operation {
            Operation::ResolvePreconditionsAndCredentials => OperationState::Binding {
                identity: plan.repository.clone(),
            },
            Operation::RunCorrectnessGate => OperationState::Gate {
                receipt: Some(plan.gate_receipt()),
            },
            Operation::BumpAndVerifyVersion => OperationState::Version {
                version: plan.requested_version.clone(),
            },
            Operation::CreateReleaseCommit => OperationState::Commit {
                head: plan.release_commit.clone(),
            },
            Operation::PublishSource => OperationState::RemoteRef {
                reference: "refs/heads/main".to_owned(),
                object: Some(plan.release_commit.clone()),
            },
            Operation::RebuildArtifacts | Operation::RehearseUpload => OperationState::Package {
                identity: Some(plan.package.clone()),
                object: Some(plan.package_object.clone()),
            },
            Operation::InstallAndProbeArtifacts => OperationState::Artifact {
                identity: Some(plan.installed_artifact.clone()),
            },
            Operation::UploadPackage => OperationState::RemoteRef {
                reference: format!("refs/spice/packages/v{}", plan.requested_version),
                object: Some(plan.package_object.clone()),
            },
            Operation::AwaitRegistryVisibility => OperationState::Registry {
                evidence: Some(RegistryEvidence {
                    version: plan.requested_version.clone(),
                    release_commit: plan.release_commit.clone(),
                    package: plan.package.clone(),
                }),
            },
            Operation::CreateImmutableTag => OperationState::RemoteRef {
                reference: format!("refs/tags/v{}", plan.requested_version),
                object: Some(plan.release_commit.clone()),
            },
            Operation::Announce => OperationState::Announcement {
                object: Some(plan.release_commit.clone()),
            },
        }
    }
}

impl EffectPort for FakeEffects {
    fn preflight(&mut self, _: &ReleasePlan) -> Result<ReleaseObservations, EffectError> {
        Ok(self.observations.clone())
    }

    fn prior(
        &mut self,
        plan: &ReleasePlan,
        operation: Operation,
    ) -> Result<OperationState, EffectError> {
        Ok(self
            .converged
            .get(&operation)
            .cloned()
            .unwrap_or_else(|| match operation {
                Operation::BumpAndVerifyVersion => OperationState::Version {
                    version: plan.observations.source_version.clone(),
                },
                Operation::CreateImmutableTag => OperationState::RemoteRef {
                    reference: format!("refs/tags/v{}", plan.requested_version),
                    object: None,
                },
                Operation::AwaitRegistryVisibility => OperationState::Registry { evidence: None },
                Operation::Announce => OperationState::Announcement { object: None },
                _ => OperationState::Artifact { identity: None },
            }))
    }

    fn converge(
        &mut self,
        plan: &ReleasePlan,
        operation: Operation,
        _: &OperationState,
    ) -> Result<(OperationState, Convergence), EffectError> {
        if self.interrupt_before == Some(operation) {
            self.interrupt_before = None;
            return Err(EffectError::Interrupted);
        }
        let existed = self.converged.contains_key(&operation);
        let result = self
            .converged
            .get(&operation)
            .cloned()
            .unwrap_or_else(|| Self::result(plan, operation));
        if !existed {
            self.calls.push(operation);
            self.converged.insert(operation, result.clone());
        }
        if self.interrupt_after == Some(operation) {
            self.interrupt_after = None;
            return Err(EffectError::Interrupted);
        }
        Ok((
            result,
            if existed {
                Convergence::AlreadyConverged
            } else {
                Convergence::Applied
            },
        ))
    }
}

#[test]
fn preview_is_pure_and_spells_the_complete_order() {
    let plan = plan_release(input(ReleaseAction::Publish)).expect("plan");
    assert_eq!(plan.operations, PUBLISH_OPERATIONS);
    assert_eq!(plan.notes, b"## Highlights\n\n- Exact curated prose.\n");
    plan.verify().expect("identity replays");
}

#[test]
fn every_bound_value_moves_the_plan_digest() {
    let baseline = plan_release(input(ReleaseAction::Publish)).expect("baseline");
    let mut changed = input(ReleaseAction::Publish);
    changed.notes.push(b'!');
    assert_ne!(
        baseline.key,
        plan_release(changed).expect("notes variant").key
    );
    let recovery = plan_release(input(ReleaseAction::TagAndAnnounce)).expect("recovery");
    assert_ne!(baseline.key, recovery.key);
    assert_eq!(recovery.operations, RECOVERY_OPERATIONS);
    assert!(!recovery.operations.contains(&Operation::RebuildArtifacts));
    assert!(!recovery.operations.contains(&Operation::UploadPackage));

    let moved = |plan: ReleasePlan| {
        let result = plan.verify();
        assert!(
            matches!(result, Err(PlanError::KeyMoved { .. })),
            "unexpected verification result: {result:?}"
        );
    };
    let mut plan = baseline.clone();
    plan.repository = digest(Domain::DistSourceCommit, b"other repository");
    moved(plan);
    let mut plan = baseline.clone();
    plan.requested_version = ProductVersion::parse("1.2.4").expect("moved version");
    moved(plan);
    let mut plan = baseline.clone();
    plan.source_commit = RELEASE.to_owned();
    plan.observations.head = RELEASE.to_owned();
    moved(plan);
    let mut plan = baseline.clone();
    plan.release_commit = SOURCE.to_owned();
    moved(plan);
    let mut plan = baseline.clone();
    plan.manifest = digest(Domain::DistReleaseManifest, b"other manifest");
    moved(plan);
    let mut plan = baseline.clone();
    plan.package = digest(Domain::DistReleasePackage, b"other package");
    moved(plan);
    let mut plan = baseline.clone();
    plan.package_object = "4444444444444444444444444444444444444444".to_owned();
    moved(plan);
    let mut plan = baseline.clone();
    plan.installed_artifact = digest(Domain::DistArtifact, b"other installed artifact");
    moved(plan);
    let mut plan = baseline.clone();
    plan.targets[0].receipt = digest(Domain::ProofReceiptV3, b"other receipt");
    moved(plan);
    let mut plan = baseline.clone();
    plan.observations.source_version = ProductVersion::parse("1.2.1").expect("moved source");
    plan.observations.release_version = plan.observations.source_version.clone();
    moved(plan);
    let mut plan = baseline;
    plan.operations.swap(0, 1);
    assert!(matches!(plan.verify(), Err(PlanError::OperationList)));
}

#[test]
fn all_no_effect_preconditions_refuse_during_planning() {
    let mut dirty = input(ReleaseAction::Publish);
    dirty.observations.clean = false;
    assert!(matches!(plan_release(dirty), Err(PlanError::Dirty)));

    let mut local = input(ReleaseAction::Publish);
    local.observations.unrecorded_local_commits = 1;
    assert!(matches!(
        plan_release(local),
        Err(PlanError::UnrecordedLocalCommits(1))
    ));

    let mut non_head = input(ReleaseAction::Publish);
    non_head.observations.head = RELEASE.to_owned();
    assert!(matches!(
        plan_release(non_head),
        Err(PlanError::NonHeadPublication { .. })
    ));

    let mut version = input(ReleaseAction::Publish);
    version.observations.release_version =
        ProductVersion::parse("1.2.1").expect("different release version");
    assert!(matches!(
        plan_release(version),
        Err(PlanError::VersionDisagreement { .. })
    ));

    let mut package = input(ReleaseAction::Publish);
    package.observations.package_ref = Some(RELEASE.to_owned());
    assert!(matches!(
        plan_release(package),
        Err(PlanError::RefMoved {
            name: "package",
            ..
        })
    ));

    let mut tag = input(ReleaseAction::Publish);
    tag.observations.tag_ref = Some(SOURCE.to_owned());
    assert!(matches!(plan_release(tag), Err(PlanError::RefMoved { .. })));

    let mut credentials = input(ReleaseAction::Publish);
    credentials.observations.credentials_valid = false;
    assert!(matches!(
        plan_release(credentials),
        Err(PlanError::Credentials)
    ));

    let mut object = input(ReleaseAction::Publish);
    object.package_object = "not-a-tree".to_owned();
    assert!(matches!(
        plan_release(object),
        Err(PlanError::Object {
            label: "package object"
        })
    ));
}

#[test]
fn recovery_requires_exact_registry_visible_evidence() {
    let mut unclaimed = input(ReleaseAction::TagAndAnnounce);
    unclaimed.observations.bump_claimed = false;
    assert!(matches!(
        plan_release(unclaimed),
        Err(PlanError::UnclaimedBump)
    ));

    let mut absent = input(ReleaseAction::TagAndAnnounce);
    absent.observations.registry = None;
    assert!(matches!(
        plan_release(absent),
        Err(PlanError::RegistryEvidenceAbsent)
    ));
    let mut moved = input(ReleaseAction::TagAndAnnounce);
    moved
        .observations
        .registry
        .as_mut()
        .expect("fixture evidence")
        .release_commit = SOURCE.to_owned();
    assert!(matches!(
        plan_release(moved),
        Err(PlanError::RegistryEvidenceMoved)
    ));
}

#[test]
fn moved_digest_and_observations_refuse_before_any_operation() {
    let plan = plan_release(input(ReleaseAction::Publish)).expect("plan");
    let mut journal = MemoryJournal::default();
    let mut effects = FakeEffects::new(&plan);
    let moved = digest(Domain::DistReleasePlan, b"moved");
    assert!(matches!(
        ReleaseExecutor::execute(&plan, &moved, &mut journal, &mut effects),
        Err(ExecuteError::AppliedKeyMoved { .. })
    ));
    assert!(effects.calls.is_empty());
    assert!(journal.accepted.is_empty());

    effects.observations.tag_ref = Some(SOURCE.to_owned());
    assert!(matches!(
        ReleaseExecutor::execute(&plan, &plan.key, &mut journal, &mut effects),
        Err(ExecuteError::ObservationsMoved)
    ));
    assert!(effects.calls.is_empty());
    assert!(journal.accepted.is_empty());
}

#[test]
fn one_executor_orders_remote_publication_after_local_proof() {
    let plan = plan_release(input(ReleaseAction::Publish)).expect("plan");
    let mut journal = MemoryJournal::default();
    let mut effects = FakeEffects::new(&plan);
    let execution = ReleaseExecutor::execute(&plan, &plan.key, &mut journal, &mut effects)
        .expect("execution converges");
    assert_eq!(effects.calls, plan.operations);
    assert_eq!(execution.completed.len(), plan.operations.len());
    ReleaseExecutor::verify_completion(&plan, &execution.completed)
        .expect("complete receipts independently verify");
    assert!(matches!(
        ReleaseExecutor::verify_completion(
            &plan,
            &execution.completed[..execution.completed.len() - 1]
        ),
        Err(ExecuteError::ReceiptCount { .. })
    ));
    let position = |operation| {
        effects
            .calls
            .iter()
            .position(|candidate| *candidate == operation)
            .expect("operation exists")
    };
    assert!(position(Operation::PublishSource) < position(Operation::UploadPackage));
    assert!(position(Operation::UploadPackage) < position(Operation::AwaitRegistryVisibility));
    assert!(position(Operation::AwaitRegistryVisibility) < position(Operation::CreateImmutableTag));
    assert!(position(Operation::CreateImmutableTag) < position(Operation::Announce));
}

#[test]
fn interruption_at_every_boundary_resumes_without_duplicate_effects() {
    let plan = plan_release(input(ReleaseAction::Publish)).expect("plan");
    for interrupted in &plan.operations {
        let mut journal = MemoryJournal::default();
        let mut effects = FakeEffects::new(&plan);
        effects.interrupt_before = Some(*interrupted);
        assert!(matches!(
            ReleaseExecutor::execute(&plan, &plan.key, &mut journal, &mut effects),
            Err(ExecuteError::Effect(EffectError::Interrupted))
        ));
        ReleaseExecutor::execute(&plan, &plan.key, &mut journal, &mut effects)
            .expect("interrupted execution resumes");
        let first_calls = effects.calls.clone();
        ReleaseExecutor::execute(&plan, &plan.key, &mut journal, &mut effects)
            .expect("completed execution replays from receipts");
        assert_eq!(effects.calls, first_calls);
        assert_eq!(effects.calls, plan.operations);
    }
    for interrupted in &plan.operations {
        let mut journal = MemoryJournal::default();
        let mut effects = FakeEffects::new(&plan);
        effects.interrupt_after = Some(*interrupted);
        assert!(matches!(
            ReleaseExecutor::execute(&plan, &plan.key, &mut journal, &mut effects),
            Err(ExecuteError::Effect(EffectError::Interrupted))
        ));
        assert!(journal.accepted.contains_key(interrupted));
        assert!(!journal.receipts.contains_key(interrupted));
        ReleaseExecutor::execute(&plan, &plan.key, &mut journal, &mut effects)
            .expect("post-effect interruption converges from accepted prior state");
        assert_eq!(effects.calls, plan.operations);
    }
}

#[test]
fn installed_and_registry_receipts_are_exact_not_recognition_only() {
    let plan = plan_release(input(ReleaseAction::Publish)).expect("plan");
    let mut journal = MemoryJournal::default();
    let mut effects = FakeEffects::new(&plan);
    effects.converged.insert(
        Operation::InstallAndProbeArtifacts,
        OperationState::Artifact {
            identity: Some(digest(Domain::DistArtifact, b"another binary")),
        },
    );
    let error = ReleaseExecutor::execute(&plan, &plan.key, &mut journal, &mut effects)
        .expect_err("wrong installed artifact refuses");
    assert!(matches!(error, ExecuteError::InstalledArtifactMoved));
    assert!(!journal.receipts.contains_key(&Operation::Announce));
}
