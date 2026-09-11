//! Native effect adapter and durable operation journal for external releases.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use wire::{Digest, Fault, FaultClass, Outcome, Refusal};

const JOURNAL_SCHEMA: u16 = 1;
const VISIBILITY_ATTEMPTS: usize = 8;

enum CorrectnessGateStanding {
    Admitted,
    Refused(String),
}

fn run_release_gate(root: &Path, source_commit: &str) -> Result<(), dist::EffectError> {
    run_release_gate_with(root, source_commit, |root, selector| {
        let report = spice::gate::run_strict_at(root, selector).map_err(|error| {
            if error.is_fault() {
                dist::EffectError::Environment(error.to_string())
            } else {
                dist::EffectError::GateRefused(error.to_string())
            }
        })?;
        if report.admits() {
            Ok(CorrectnessGateStanding::Admitted)
        } else {
            Ok(CorrectnessGateStanding::Refused(
                report.refusal_fold().to_string(),
            ))
        }
    })
}

fn run_release_gate_with(
    root: &Path,
    source_commit: &str,
    runner: impl FnOnce(&Path, gate::Selector) -> Result<CorrectnessGateStanding, dist::EffectError>,
) -> Result<(), dist::EffectError> {
    match runner(root, gate::Selector::revision(source_commit))? {
        CorrectnessGateStanding::Admitted => Ok(()),
        CorrectnessGateStanding::Refused(detail) => Err(dist::EffectError::GateRefused(detail)),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct JournalDocument {
    schema: u16,
    plan: dist::ReleasePlan,
    accepted: Vec<dist::AcceptedOperation>,
    receipts: Vec<dist::OperationReceipt>,
}

pub(super) struct FileReleaseJournal {
    path: PathBuf,
    document: JournalDocument,
}

impl FileReleaseJournal {
    fn open(root: &Path, plan: &dist::ReleasePlan) -> Result<Self, String> {
        let path = root.join(format!("{}.json", plan.key));
        let document = match fs::read(&path) {
            Ok(bytes) => {
                let document: JournalDocument =
                    wire::decode(&bytes).map_err(|error| error.to_string())?;
                let canonical = wire::encode(&document)
                    .map_err(|error| format!("journal encoding: {error}"))?;
                if canonical != bytes {
                    return Err("release operation journal is not canonical".to_owned());
                }
                if document.schema != JOURNAL_SCHEMA || document.plan != *plan {
                    return Err("release operation journal identity moved".to_owned());
                }
                validate_document(&document)?;
                document
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => JournalDocument {
                schema: JOURNAL_SCHEMA,
                plan: plan.clone(),
                accepted: Vec::new(),
                receipts: Vec::new(),
            },
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        Ok(Self { path, document })
    }

    fn persist(&self) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "release journal has no parent".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        let bytes = wire::encode(&self.document).map_err(|error| error.to_string())?;
        let mut staged = atomic::Destination::new(&self.path)
            .map_err(|error| error.to_string())?
            .stage()
            .map_err(|error| error.to_string())?;
        staged
            .write_all(&bytes)
            .map_err(|error| format!("cannot write {}: {error}", self.path.display()))?;
        staged.commit().map_err(|error| error.to_string())
    }
}

fn validate_document(document: &JournalDocument) -> Result<(), String> {
    document.plan.verify().map_err(|error| error.to_string())?;
    if document.accepted.len() > document.plan.operations.len()
        || document
            .accepted
            .iter()
            .zip(&document.plan.operations)
            .any(|(accepted, operation)| accepted.operation != *operation)
        || document.receipts.len() > document.accepted.len()
        || document.accepted.len() > document.receipts.len().saturating_add(1)
        || document
            .receipts
            .iter()
            .zip(&document.plan.operations)
            .any(|(receipt, operation)| receipt.operation != *operation)
    {
        return Err("release operation journal is incomplete or reordered".to_owned());
    }
    for (index, accepted) in document.accepted.iter().enumerate() {
        if accepted.plan != document.plan.key
            || document.accepted[..index]
                .iter()
                .any(|prior| prior.operation == accepted.operation)
        {
            return Err("release operation journal has a moved or duplicate acceptance".to_owned());
        }
    }
    for (index, receipt) in document.receipts.iter().enumerate() {
        let accepted = document
            .accepted
            .iter()
            .find(|accepted| accepted.operation == receipt.operation);
        if receipt.plan != document.plan.key
            || document.receipts[..index]
                .iter()
                .any(|prior| prior.operation == receipt.operation)
            || !matches!(accepted, Some(accepted) if accepted.prior == receipt.prior)
        {
            return Err("release operation journal has a moved or duplicate receipt".to_owned());
        }
    }
    Ok(())
}

impl dist::ReleaseJournal for FileReleaseJournal {
    type Error = String;

    fn accepted(
        &mut self,
        plan: &Digest,
        operation: dist::Operation,
    ) -> Result<Option<dist::AcceptedOperation>, Self::Error> {
        Ok(self
            .document
            .accepted
            .iter()
            .find(|row| row.plan == *plan && row.operation == operation)
            .cloned())
    }

    fn receipt(
        &mut self,
        plan: &Digest,
        operation: dist::Operation,
    ) -> Result<Option<dist::OperationReceipt>, Self::Error> {
        Ok(self
            .document
            .receipts
            .iter()
            .find(|row| row.plan == *plan && row.operation == operation)
            .cloned())
    }

    fn accept(&mut self, accepted: &dist::AcceptedOperation) -> Result<(), Self::Error> {
        if self
            .document
            .accepted
            .iter()
            .any(|row| row.operation == accepted.operation)
        {
            return Err(format!(
                "operation {:?} was accepted twice",
                accepted.operation
            ));
        }
        self.document.accepted.push(accepted.clone());
        self.persist()
    }

    fn complete(&mut self, receipt: &dist::OperationReceipt) -> Result<(), Self::Error> {
        if !self
            .document
            .accepted
            .iter()
            .any(|row| row.operation == receipt.operation && row.prior == receipt.prior)
        {
            return Err(format!(
                "operation {:?} completed without its exact accepted prior state",
                receipt.operation
            ));
        }
        if self
            .document
            .receipts
            .iter()
            .any(|row| row.operation == receipt.operation)
        {
            return Err(format!(
                "operation {:?} was receipted twice",
                receipt.operation
            ));
        }
        self.document.receipts.push(receipt.clone());
        self.persist()
    }
}

pub(super) struct SystemEffects {
    root: PathBuf,
    probe_root: PathBuf,
    authority: dist::ReleaseAuthority,
    appliance: PathBuf,
}

impl SystemEffects {
    fn new(root: &Path, journal_root: &Path) -> Result<Self, dist::EffectError> {
        let authority = dist::current_authority(root)
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        let repository = spice::repository::Repository::discover(root)
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        let probe_root = journal_root
            .parent()
            .unwrap_or(journal_root)
            .join("release-probes");
        Ok(Self {
            root: root.to_path_buf(),
            probe_root,
            authority,
            appliance: spice::install::installed_binary(&repository),
        })
    }

    fn remote_ref(&self, reference: &str) -> Result<Option<String>, dist::EffectError> {
        let command = self
            .authority
            .reference_visibility(reference)
            .map_err(|error| dist::EffectError::RemoteRefused(error.to_string()))?;
        let output = self.git_owned(&command)?;
        Ok(output.split_whitespace().next().map(str::to_owned))
    }

    fn git_local(&self, arguments: &[&str]) -> Result<String, dist::EffectError> {
        run(&self.root, "git", arguments).map_err(dist::EffectError::Environment)
    }

    fn git_owned(&self, command: &dist::GitCommand) -> Result<String, dist::EffectError> {
        let arguments = command
            .arguments()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        run(&self.root, command.program(), &arguments).map_err(dist::EffectError::RemoteRefused)
    }

    fn reference_for(
        &self,
        plan: &dist::ReleasePlan,
        operation: dist::Operation,
    ) -> Option<String> {
        let refs = self.authority.refs(&plan.requested_version);
        match operation {
            dist::Operation::PublishSource => Some(refs.source().to_owned()),
            dist::Operation::RehearseUpload | dist::Operation::UploadPackage => {
                Some(refs.package().to_owned())
            }
            dist::Operation::CreateImmutableTag => Some(refs.tag().to_owned()),
            dist::Operation::Announce => Some(refs.announcement().to_owned()),
            _ => None,
        }
    }

    fn registry_state(
        &self,
        plan: &dist::ReleasePlan,
    ) -> Result<dist::OperationState, dist::EffectError> {
        let reference = self
            .authority
            .refs(&plan.requested_version)
            .package()
            .to_owned();
        let evidence = match self.remote_ref(&reference)? {
            Some(object) if object == plan.package_object => Some(dist::RegistryEvidence {
                version: plan.requested_version.clone(),
                release_commit: plan.release_commit.clone(),
                package: plan.package.clone(),
            }),
            Some(object) => {
                return Err(dist::EffectError::RemoteRefused(format!(
                    "{reference} names {object}, not {}",
                    plan.package_object
                )));
            }
            None => None,
        };
        Ok(dist::OperationState::Registry { evidence })
    }

    fn probe_path(&self, plan: &dist::ReleasePlan) -> PathBuf {
        self.probe_root.join(plan.key.as_str()).join("spice")
    }

    fn installed_state(
        &self,
        plan: &dist::ReleasePlan,
    ) -> Result<dist::OperationState, dist::EffectError> {
        let path = self.probe_path(plan);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                let bytes = fs::read(path)
                    .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
                Ok(dist::OperationState::Artifact {
                    identity: Some(Digest::compute(wire::Domain::DistArtifact, &bytes)),
                })
            }
            Ok(_) => Err(dist::EffectError::Moved(
                "independent release probe is not a regular file".to_owned(),
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(dist::OperationState::Artifact { identity: None })
            }
            Err(error) => Err(dist::EffectError::Environment(error.to_string())),
        }
    }

    fn gate_state(
        &self,
        plan: &dist::ReleasePlan,
    ) -> Result<dist::OperationState, dist::EffectError> {
        let path = self
            .probe_root
            .join(plan.key.as_str())
            .join("correctness-gate.receipt");
        let receipt = match fs::read(&path) {
            Ok(bytes) => Some(
                String::from_utf8(bytes)
                    .map_err(|error| dist::EffectError::Environment(error.to_string()))?
                    .trim()
                    .parse::<Digest>()
                    .map_err(|error| dist::EffectError::Environment(error.to_string()))?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(dist::EffectError::Environment(error.to_string())),
        };
        Ok(dist::OperationState::Gate { receipt })
    }

    fn run_dev_xtask(&self, target: &str) -> Result<String, String> {
        let appliance = self
            .appliance
            .to_str()
            .ok_or_else(|| "installed Spice path is not UTF-8".to_owned())?;
        run(&self.root, appliance, &dev_xtask_arguments(target))
    }

    fn rehearse_source(&self, object: &str) -> Result<(), dist::EffectError> {
        let command = self
            .authority
            .source_rehearsal(object)
            .map_err(|error| dist::EffectError::RemoteRefused(error.to_string()))?;
        self.git_owned(&command).map(|_| ())
    }

    fn install_and_probe(
        &self,
        plan: &dist::ReleasePlan,
    ) -> Result<(dist::OperationState, dist::Convergence), dist::EffectError> {
        let target = proof::compiled_target()
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        let source = self
            .root
            .join("dist/stage/targets")
            .join(target.as_str())
            .join("bin/spice");
        let bytes =
            fs::read(&source).map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        let identity = Digest::compute(wire::Domain::DistArtifact, &bytes);
        if identity != plan.installed_artifact {
            return Err(dist::EffectError::Moved(format!(
                "staged executable {} moved",
                source.display()
            )));
        }
        let path = self.probe_path(plan);
        let prior = self.installed_state(plan)?;
        let convergence = if prior
            == (dist::OperationState::Artifact {
                identity: Some(identity.clone()),
            }) {
            dist::Convergence::AlreadyConverged
        } else {
            let parent = path
                .parent()
                .expect("release probe path has a fixed parent");
            fs::create_dir_all(parent)
                .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
            let mut staged = atomic::Destination::new(&path)
                .map_err(|error| dist::EffectError::Environment(error.to_string()))?
                .stage()
                .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
            staged
                .write_all(&bytes)
                .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
            make_executable(staged.temporary_path())?;
            staged
                .commit()
                .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
            dist::Convergence::Applied
        };
        run(
            &self.root,
            path.to_string_lossy().as_ref(),
            &["--json", "version"],
        )
        .map_err(dist::EffectError::GateRefused)?;
        Ok((
            dist::OperationState::Artifact {
                identity: Some(identity),
            },
            convergence,
        ))
    }
}

impl dist::EffectPort for SystemEffects {
    fn preflight(
        &mut self,
        plan: &dist::ReleasePlan,
    ) -> Result<dist::ReleaseObservations, dist::EffectError> {
        let mut observed = plan.observations.clone();
        let repository =
            crate::release_command::repository_identity(&self.root, self.authority.remote())
                .map_err(dist::EffectError::Environment)?;
        if repository != plan.repository {
            return Err(dist::EffectError::Moved(
                "release repository authority changed".to_owned(),
            ));
        }
        let head = self.git_local(&["rev-parse", "--verify", "HEAD^{commit}"])?;
        let status = self.git_local(&["status", "--porcelain=v1", "--untracked-files=no"])?;
        let version = fs::read_to_string(self.root.join("dist/VERSION"))
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        let version = dist::ProductVersion::parse(version.trim())
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        if !admitted_local_progress(plan, &head, &version, &status) {
            observed.head = head.clone();
            observed.clean = status.is_empty();
            observed.release_version = version;
        }
        let credential_object = if object_exists(&self.root, &plan.release_commit)? {
            &plan.release_commit
        } else {
            &plan.source_commit
        };
        self.rehearse_source(credential_object)?;
        let refs = self.authority.refs(&plan.requested_version);
        observed.source_ref = stable_source_observation(
            plan.observations.source_ref.as_ref(),
            self.remote_ref(refs.source())?,
            &plan.source_commit,
            &plan.release_commit,
        );
        observed.package_ref = stable_ref_observation(
            plan.observations.package_ref.as_ref(),
            self.remote_ref(refs.package())?,
            &plan.package_object,
        );
        observed.tag_ref = stable_ref_observation(
            plan.observations.tag_ref.as_ref(),
            self.remote_ref(refs.tag())?,
            &plan.release_commit,
        );
        observed.announcement_ref = stable_ref_observation(
            plan.observations.announcement_ref.as_ref(),
            self.remote_ref(refs.announcement())?,
            &plan.release_commit,
        );
        observed.credentials_valid = true;
        if plan.action == dist::ReleaseAction::TagAndAnnounce {
            let dist::OperationState::Registry { evidence } = self.registry_state(plan)? else {
                unreachable!("registry observer has one state shape")
            };
            observed.registry = evidence;
        }
        let package = package_preview(&self.root, plan)?;
        if package.identity != plan.package || package.git_tree != plan.package_object {
            return Err(dist::EffectError::Moved(
                "staged package identity or Git tree changed".to_owned(),
            ));
        }
        Ok(observed)
    }

    fn prior(
        &mut self,
        plan: &dist::ReleasePlan,
        operation: dist::Operation,
    ) -> Result<dist::OperationState, dist::EffectError> {
        use dist::Operation as Op;
        match operation {
            Op::ResolvePreconditionsAndCredentials => {
                let identity = crate::release_command::repository_identity(
                    &self.root,
                    self.authority.remote(),
                )
                .map_err(dist::EffectError::Environment)?;
                Ok(dist::OperationState::Binding { identity })
            }
            Op::RunCorrectnessGate => self.gate_state(plan),
            Op::BumpAndVerifyVersion => {
                let version = fs::read_to_string(self.root.join("dist/VERSION"))
                    .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
                Ok(dist::OperationState::Version {
                    version: dist::ProductVersion::parse(version.trim())
                        .map_err(|error| dist::EffectError::Environment(error.to_string()))?,
                })
            }
            Op::CreateReleaseCommit => Ok(dist::OperationState::Commit {
                head: self.git_local(&["rev-parse", "--verify", "HEAD^{commit}"])?,
            }),
            Op::RebuildArtifacts | Op::RehearseUpload => {
                let package = package_preview(&self.root, plan)?;
                Ok(dist::OperationState::Package {
                    identity: Some(package.identity),
                    object: object_exists(&self.root, &package.git_tree)?
                        .then_some(package.git_tree),
                })
            }
            Op::InstallAndProbeArtifacts => self.installed_state(plan),
            Op::AwaitRegistryVisibility => self.registry_state(plan),
            Op::PublishSource | Op::UploadPackage | Op::CreateImmutableTag => {
                let reference = self
                    .reference_for(plan, operation)
                    .expect("remote-ref operation has one authority ref");
                Ok(dist::OperationState::RemoteRef {
                    object: self.remote_ref(&reference)?,
                    reference,
                })
            }
            Op::Announce => {
                let reference = self
                    .reference_for(plan, operation)
                    .expect("announcement has one authority ref");
                Ok(dist::OperationState::Announcement {
                    object: self.remote_ref(&reference)?,
                })
            }
        }
    }

    fn converge(
        &mut self,
        plan: &dist::ReleasePlan,
        operation: dist::Operation,
        prior: &dist::OperationState,
    ) -> Result<(dist::OperationState, dist::Convergence), dist::EffectError> {
        use dist::Operation as Op;
        match operation {
            Op::ResolvePreconditionsAndCredentials => self.converge_preconditions(plan),
            Op::RunCorrectnessGate => self.converge_gate(plan, prior),
            Op::BumpAndVerifyVersion => self.converge_version(plan, prior),
            Op::CreateReleaseCommit => self.converge_commit(plan, prior),
            Op::RebuildArtifacts => self.converge_rebuild(plan, prior),
            Op::InstallAndProbeArtifacts => self.install_and_probe(plan),
            Op::RehearseUpload => self.converge_rehearsal(plan),
            Op::PublishSource => self.converge_source(plan, prior),
            Op::UploadPackage | Op::CreateImmutableTag | Op::Announce => {
                self.converge_immutable(plan, operation, prior)
            }
            Op::AwaitRegistryVisibility => self.await_registry(plan),
        }
    }
}

impl SystemEffects {
    fn converge_preconditions(
        &self,
        plan: &dist::ReleasePlan,
    ) -> Result<(dist::OperationState, dist::Convergence), dist::EffectError> {
        let identity =
            crate::release_command::repository_identity(&self.root, self.authority.remote())
                .map_err(dist::EffectError::Environment)?;
        if identity != plan.repository {
            return Err(dist::EffectError::Moved(
                "release repository authority changed".to_owned(),
            ));
        }
        let object = if object_exists(&self.root, &plan.release_commit)? {
            &plan.release_commit
        } else {
            &plan.source_commit
        };
        self.rehearse_source(object)?;
        Ok((
            dist::OperationState::Binding { identity },
            dist::Convergence::Applied,
        ))
    }

    fn converge_gate(
        &self,
        plan: &dist::ReleasePlan,
        prior: &dist::OperationState,
    ) -> Result<(dist::OperationState, dist::Convergence), dist::EffectError> {
        let current = self.gate_state(plan)?;
        if current
            == (dist::OperationState::Gate {
                receipt: Some(plan.gate_receipt()),
            })
        {
            return Ok((current, dist::Convergence::AlreadyConverged));
        }
        if matches!(current, dist::OperationState::Gate { receipt: Some(_) }) {
            return Err(dist::EffectError::Moved(
                "correctness gate receipt identity changed".to_owned(),
            ));
        }
        if matches!(prior, dist::OperationState::Gate { receipt: Some(_) }) {
            return Ok((prior.clone(), dist::Convergence::AlreadyConverged));
        }
        if !matches!(prior, dist::OperationState::Gate { receipt: None }) {
            return Err(dist::EffectError::Environment(
                "correctness gate received the wrong prior-state shape".to_owned(),
            ));
        }
        run_release_gate(&self.root, &plan.source_commit)?;
        let head = self.git_local(&["rev-parse", "--verify", "HEAD^{commit}"])?;
        let status = self.git_local(&["status", "--porcelain=v1", "--untracked-files=no"])?;
        if head != plan.source_commit || !status.is_empty() {
            return Err(dist::EffectError::GateRefused(format!(
                "correctness gate moved source state: HEAD={head} status={status:?}"
            )));
        }
        let receipt = plan.gate_receipt();
        let path = self
            .probe_root
            .join(plan.key.as_str())
            .join("correctness-gate.receipt");
        let parent = path.parent().expect("gate receipt has one parent");
        fs::create_dir_all(parent)
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        let mut staged = atomic::Destination::new(&path)
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?
            .stage()
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        staged
            .write_all(format!("{receipt}\n").as_bytes())
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        staged
            .commit()
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        Ok((
            dist::OperationState::Gate {
                receipt: Some(receipt),
            },
            dist::Convergence::Applied,
        ))
    }

    fn converge_version(
        &self,
        plan: &dist::ReleasePlan,
        prior: &dist::OperationState,
    ) -> Result<(dist::OperationState, dist::Convergence), dist::EffectError> {
        let dist::OperationState::Version { version: prior } = prior else {
            return Err(dist::EffectError::Environment(
                "version bump received the wrong prior-state shape".to_owned(),
            ));
        };
        let path = self.root.join("dist/VERSION");
        let current = fs::read_to_string(&path)
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        let current = dist::ProductVersion::parse(current.trim())
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        if current == plan.requested_version {
            return Ok((
                dist::OperationState::Version { version: current },
                dist::Convergence::AlreadyConverged,
            ));
        }
        if &current != prior || current != plan.observations.source_version {
            return Err(dist::EffectError::Moved(format!(
                "release version moved from {prior} to {current}"
            )));
        }
        let mut staged = atomic::Destination::new(&path)
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?
            .stage()
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        staged
            .write_all(format!("{}\n", plan.requested_version).as_bytes())
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        staged
            .commit()
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        let observed = fs::read_to_string(&path)
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        let observed = dist::ProductVersion::parse(observed.trim())
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        if observed != plan.requested_version {
            return Err(dist::EffectError::Moved(
                "version bump did not converge to the requested version".to_owned(),
            ));
        }
        Ok((
            dist::OperationState::Version { version: observed },
            dist::Convergence::Applied,
        ))
    }

    fn converge_commit(
        &self,
        plan: &dist::ReleasePlan,
        prior: &dist::OperationState,
    ) -> Result<(dist::OperationState, dist::Convergence), dist::EffectError> {
        let dist::OperationState::Commit { head: prior_head } = prior else {
            return Err(dist::EffectError::Environment(
                "release commit received the wrong prior-state shape".to_owned(),
            ));
        };
        let current = self.git_local(&["rev-parse", "--verify", "HEAD^{commit}"])?;
        if current == plan.release_commit {
            return Ok((
                dist::OperationState::Commit { head: current },
                dist::Convergence::AlreadyConverged,
            ));
        }
        if prior_head != &plan.source_commit || current != plan.source_commit {
            return Err(dist::EffectError::Moved(format!(
                "release HEAD moved from {} to {current}",
                plan.source_commit
            )));
        }
        let version = fs::read_to_string(self.root.join("dist/VERSION"))
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        let version = dist::ProductVersion::parse(version.trim())
            .map_err(|error| dist::EffectError::Environment(error.to_string()))?;
        let status = self.git_local(&["status", "--porcelain=v1", "--untracked-files=no"])?;
        let bump_is_exact = if plan.requested_version == plan.observations.source_version {
            status.is_empty()
        } else {
            exact_version_bump(&status)
        };
        if version != plan.requested_version || !bump_is_exact {
            return Err(dist::EffectError::Moved(format!(
                "release bump moved before commit: version={version} status={status:?}"
            )));
        }
        let fold = crate::release_commit::project(
            &self.root,
            &plan.source_commit,
            &plan.requested_version,
        )
        .map_err(dist::EffectError::Environment)?;
        if fold.commit != plan.release_commit {
            return Err(dist::EffectError::Moved(
                "deterministic release commit identity changed".to_owned(),
            ));
        }
        let head = crate::release_commit::materialize(&self.root, &fold, &plan.requested_version)
            .map_err(dist::EffectError::Environment)?;
        Ok((
            dist::OperationState::Commit { head },
            dist::Convergence::Applied,
        ))
    }

    fn converge_rebuild(
        &mut self,
        plan: &dist::ReleasePlan,
        prior: &dist::OperationState,
    ) -> Result<(dist::OperationState, dist::Convergence), dist::EffectError> {
        for target in &plan.targets {
            self.run_dev_xtask(&target.target)
                .map_err(dist::EffectError::Environment)?;
        }
        let package = crate::release_package::materialize(
            &self.root,
            &plan.requested_version,
            plan.notes_identity.clone(),
            &plan.notes,
            &manifest_bytes(&self.root, plan)?,
        )
        .map_err(|error| match error {
            crate::release_package::PackageFailure::Moved(detail) => {
                dist::EffectError::Moved(detail)
            }
            crate::release_package::PackageFailure::Environment(detail) => {
                dist::EffectError::Environment(detail)
            }
        })?;
        if package.identity != plan.package || package.git_tree != plan.package_object {
            return Err(dist::EffectError::Moved(
                "rebuilt package identity or Git tree changed".to_owned(),
            ));
        }
        let convergence = if matches!(
            prior,
            dist::OperationState::Package {
                object: Some(object),
                ..
            } if object == &plan.package_object
        ) {
            dist::Convergence::AlreadyConverged
        } else {
            dist::Convergence::Applied
        };
        Ok((
            package_state(package.identity, package.git_tree),
            convergence,
        ))
    }

    fn converge_rehearsal(
        &mut self,
        plan: &dist::ReleasePlan,
    ) -> Result<(dist::OperationState, dist::Convergence), dist::EffectError> {
        let command = self
            .authority
            .package_rehearsal(&plan.requested_version, &plan.package_object)
            .map_err(|error| dist::EffectError::RemoteRefused(error.to_string()))?;
        self.git_owned(&command)?;
        Ok((
            package_state(plan.package.clone(), plan.package_object.clone()),
            dist::Convergence::Applied,
        ))
    }

    fn converge_source(
        &mut self,
        plan: &dist::ReleasePlan,
        prior: &dist::OperationState,
    ) -> Result<(dist::OperationState, dist::Convergence), dist::EffectError> {
        let reference = self
            .authority
            .refs(&plan.requested_version)
            .source()
            .to_owned();
        if !matches!(prior, dist::OperationState::RemoteRef { .. }) {
            return Err(dist::EffectError::Environment(
                "publish source received the wrong prior-state shape".to_owned(),
            ));
        }
        match self.remote_ref(&reference)? {
            Some(object) if object == plan.release_commit => {
                return Ok((
                    dist::OperationState::RemoteRef {
                        reference,
                        object: Some(object),
                    },
                    dist::Convergence::AlreadyConverged,
                ));
            }
            Some(object) if object != plan.source_commit => {
                return Err(dist::EffectError::RemoteRefused(format!(
                    "{reference} moved to {object} after preflight"
                )));
            }
            Some(_) | None => {}
        }
        let command = self
            .authority
            .source_push(&plan.release_commit)
            .map_err(|error| dist::EffectError::RemoteRefused(error.to_string()))?;
        self.git_owned(&command)?;
        Ok((
            dist::OperationState::RemoteRef {
                reference,
                object: Some(plan.release_commit.clone()),
            },
            dist::Convergence::Applied,
        ))
    }

    fn await_registry(
        &self,
        plan: &dist::ReleasePlan,
    ) -> Result<(dist::OperationState, dist::Convergence), dist::EffectError> {
        for _ in 0..VISIBILITY_ATTEMPTS {
            let observed = self.registry_state(plan)?;
            if matches!(
                observed,
                dist::OperationState::Registry { evidence: Some(_) }
            ) {
                return Ok((observed, dist::Convergence::AlreadyConverged));
            }
        }
        Err(dist::EffectError::RemoteRefused(format!(
            "package v{} did not become visible within {VISIBILITY_ATTEMPTS} responses",
            plan.requested_version
        )))
    }

    fn converge_immutable(
        &self,
        plan: &dist::ReleasePlan,
        operation: dist::Operation,
        prior: &dist::OperationState,
    ) -> Result<(dist::OperationState, dist::Convergence), dist::EffectError> {
        match prior {
            dist::OperationState::RemoteRef { .. } | dist::OperationState::Announcement { .. } => {}
            _ => {
                return Err(dist::EffectError::Environment(format!(
                    "{operation:?} received the wrong prior-state shape"
                )));
            }
        }
        let reference = self
            .reference_for(plan, operation)
            .expect("immutable operation has one authority ref");
        let current = self.remote_ref(&reference)?;
        let mutation = match operation {
            dist::Operation::UploadPackage => self.authority.package_push(
                &plan.requested_version,
                &plan.package_object,
                ref_state(current.as_deref()),
            ),
            dist::Operation::CreateImmutableTag => self.authority.tag_push(
                &plan.requested_version,
                &plan.release_commit,
                ref_state(current.as_deref()),
            ),
            dist::Operation::Announce => self.authority.announcement_push(
                &plan.requested_version,
                &plan.release_commit,
                ref_state(current.as_deref()),
            ),
            _ => unreachable!("caller admits only immutable-ref operations"),
        }
        .map_err(|error| dist::EffectError::RemoteRefused(error.to_string()))?;
        let convergence = match mutation {
            dist::GitMutation::Converged => dist::Convergence::AlreadyConverged,
            dist::GitMutation::Command(command) => {
                self.git_owned(&command)?;
                dist::Convergence::Applied
            }
        };
        let target_object = match operation {
            dist::Operation::UploadPackage => &plan.package_object,
            dist::Operation::CreateImmutableTag | dist::Operation::Announce => &plan.release_commit,
            _ => unreachable!("caller admits only immutable-ref operations"),
        };
        let result = match operation {
            dist::Operation::Announce => dist::OperationState::Announcement {
                object: Some(target_object.to_owned()),
            },
            _ => dist::OperationState::RemoteRef {
                reference,
                object: Some(target_object.to_owned()),
            },
        };
        Ok((result, convergence))
    }
}

fn dev_xtask_arguments(target: &str) -> [&str; 5] {
    ["dev", "xtask", "--", "stage-target", target]
}

mod tail;
#[cfg(test)]
use tail::effect_outcome;
use tail::{
    admitted_local_progress, exact_version_bump, make_executable, manifest_bytes, object_exists,
    package_preview, package_state, ref_state, run, stable_ref_observation,
    stable_source_observation,
};
pub(crate) use tail::{execute, verify_completed};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_rebuilds_both_targets_through_the_admitted_dev_xtask_boundary() {
        for target in [
            wire::manifest::ReleaseTarget::Aarch64AppleDarwin,
            wire::manifest::ReleaseTarget::X86_64UnknownLinuxMusl,
        ] {
            assert_eq!(
                dev_xtask_arguments(target.as_str()),
                ["dev", "xtask", "--", "stage-target", target.as_str()]
            );
        }
    }

    #[test]
    fn external_effect_failures_keep_their_operator_visible_categories() {
        let refusals = [
            (
                dist::EffectError::Moved("binding".to_owned()),
                "release::external-plan-moved",
            ),
            (
                dist::EffectError::Interrupted,
                "release::operator-interrupted",
            ),
            (
                dist::EffectError::GateRefused("gate".to_owned()),
                "release::gate-refused",
            ),
            (
                dist::EffectError::RemoteRefused("remote".to_owned()),
                "release::remote-refused",
            ),
        ];
        for (error, expected) in refusals {
            let Outcome::Refusal(refusal) = effect_outcome(error) else {
                panic!("{expected} must remain an operator refusal")
            };
            assert_eq!(refusal.reason(), expected);
        }

        let Outcome::Fault(fault) =
            effect_outcome(dist::EffectError::Environment("filesystem".to_owned()))
        else {
            panic!("environment failure must remain a fault")
        };
        assert_eq!(fault.class, FaultClass::Environment);
        assert_eq!(fault.reason, "release::environment-failed");
    }

    #[test]
    fn recognition_verification_cannot_bypass_a_missing_external_journal() {
        let journal = std::env::temp_dir().join(format!(
            "spice-absent-release-journal-{}",
            std::process::id()
        ));
        let key = Digest::compute(wire::Domain::DistReleasePlan, b"missing plan");
        let manifest = Digest::compute(wire::Domain::DistReleaseManifest, b"missing manifest");
        let notes = Digest::compute(wire::Domain::DistReleaseNotes, b"missing notes");

        let error = verify_completed(&journal, &key, &manifest, &notes, &[])
            .expect_err("resident recognition requires the external receipt chain");
        assert!(error.contains("cannot read"));
    }

    #[test]
    fn release_gate_binds_the_explicit_root_and_source_and_refuses_red() {
        let root = Path::new("/fixture/release-root");
        let source = "0123456789abcdef0123456789abcdef01234567";
        run_release_gate_with(root, source, |observed_root, selector| {
            assert_eq!(observed_root, root);
            assert_eq!(selector, gate::Selector::revision(source));
            Ok(CorrectnessGateStanding::Admitted)
        })
        .expect("an admitted canonical report crosses the release gate");

        let refused = run_release_gate_with(root, "source", |_, _| {
            Ok(CorrectnessGateStanding::Refused("red arm".to_owned()))
        })
        .expect_err("a red canonical report cannot mint a release receipt");
        assert!(matches!(
            refused,
            dist::EffectError::GateRefused(detail) if detail == "red arm"
        ));

        let unavailable = run_release_gate_with(root, "source", |_, _| {
            Err(dist::EffectError::Environment(
                "gate unavailable".to_owned(),
            ))
        })
        .expect_err("a gate acquisition fault stays an environment fault");
        assert!(matches!(
            unavailable,
            dist::EffectError::Environment(detail) if detail == "gate unavailable"
        ));
    }
}
