//! Bounded acquisition for the daemonless Doctor report.

mod subsystems;

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use spice::config::{self, Resolved, Root};
use spice::repository::Repository;
use wire::{
    DoctorCommand, DoctorConfigProvenance, DoctorEnvironment, DoctorExtension,
    DoctorExtensionNamespace, DoctorLocations, DoctorObservation, DoctorReport, DoctorTarget,
    DoctorTool, DoctorToolSource, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition,
    SpiceVariable,
};

use crate::cli::{self, ExternalTool};

/// Inspect the repository containing `working` without opening authority or
/// launching any required command tool. The optional configured RTK executable
/// crosses its bounded health contract because availability alone is unsafe.
pub(crate) fn inspect(
    working: &Path,
    environment: &[(SpiceVariable, bool)],
    search_path: Option<&OsStr>,
    configuration: Option<&config::RepositoryCapture>,
) -> Outcome<DoctorReport<Root>> {
    #[cfg(debug_assertions)]
    spice::install::configure_test_maintenance_now(crate::bootstrap_environment(
        spice::install::TEST_PAUSE_ACTION,
    ));
    let repository = match Repository::discover(working) {
        Ok(repository) => repository,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "doctor::repository-unresolved".to_owned(),
                detail: error.to_string(),
            });
        }
    };
    let loaded = match config::capture_repository_scoped(&repository, &[], configuration) {
        Ok(loaded) => loaded,
        Err(error) => return Outcome::Refusal(configuration_refusal(&error)),
    };
    let resolved = loaded.resolved().clone();
    let probe = FilesystemProbe::new(working, search_path);

    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: report(working, &repository, resolved, environment, &probe),
    })
}

fn report(
    working: &Path,
    repository: &Repository,
    resolved: Resolved,
    environment: &[(SpiceVariable, bool)],
    probe: &impl ExecutableProbe,
) -> DoctorReport<Root> {
    let locations = config::locations(repository);
    let provenance = resolved
        .provenance()
        .iter()
        .map(|row| DoctorConfigProvenance {
            path: row.path().to_string(),
            source: row.source(),
        })
        .collect();
    let configuration = resolved.into_root();
    let commands = cli::contracts()
        .iter()
        .map(|contract| command_report(contract, &configuration, probe))
        .collect();

    DoctorReport {
        target: DoctorTarget {
            os: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
        },
        locations: DoctorLocations {
            working: working.display().to_string(),
            repository: repository.worktree_root().display().to_string(),
            repository_common: repository.common_dir().display().to_string(),
            tracked_configuration: locations.tracked().display().to_string(),
            worktree_configuration: locations.worktree().display().to_string(),
            local_configuration: locations.local().display().to_string(),
        },
        environment: SpiceVariable::ALL
            .iter()
            .map(|variable| DoctorEnvironment {
                variable: *variable,
                observation: if environment
                    .iter()
                    .any(|(candidate, present)| candidate == variable && *present)
                {
                    DoctorObservation::Ready {}
                } else {
                    DoctorObservation::Unavailable {
                        reason: format!("{} is absent", variable.as_str()),
                    }
                },
            })
            .collect(),
        subsystems: subsystems::diagnose(repository, &configuration),
        configuration,
        provenance,
        commands,
        extensions: extension_report(spice::extension::DECLARATIONS),
    }
}

fn extension_report(declarations: &[spice::extension::Declaration]) -> Vec<DoctorExtension> {
    match spice::extension::Registry::discover(declarations) {
        Ok(registry) => registry
            .rows()
            .iter()
            .map(|row| DoctorExtension {
                namespace: extension_namespace(row.namespace),
                name: row.name.to_owned(),
                provider: row.provider.to_owned(),
                observation: DoctorObservation::Ready {},
            })
            .collect(),
        Err(error) => vec![DoctorExtension {
            namespace: extension_namespace(error.namespace()),
            name: "registry".to_owned(),
            provider: "spice".to_owned(),
            observation: DoctorObservation::Unsupported {
                reason: error.to_string(),
            },
        }],
    }
}

const fn extension_namespace(namespace: spice::extension::Namespace) -> DoctorExtensionNamespace {
    match namespace {
        spice::extension::Namespace::Vendor => DoctorExtensionNamespace::Vendor,
        spice::extension::Namespace::Wrapper => DoctorExtensionNamespace::Wrapper,
        spice::extension::Namespace::Gate => DoctorExtensionNamespace::Gate,
    }
}

fn command_report(
    contract: &cli::Contract,
    configuration: &Root,
    probe: &impl ExecutableProbe,
) -> DoctorCommand {
    let tools: Vec<DoctorTool> = contract
        .tools
        .iter()
        .map(|tool| DoctorTool {
            name: tool.name.to_owned(),
            owner: tool.owner.to_owned(),
            mode: tool.mode,
            requirement: tool.requirement,
            source: tool.source,
            bounds: tool.bounds,
            cancellation: tool.cancellation,
            network: tool.network,
            observation: tool_observation(tool, configuration, probe),
        })
        .collect();
    let observation = tools
        .iter()
        .find(|tool| !matches!(tool.observation, DoctorObservation::Ready {}))
        .map_or_else(
            || {
                if tools.is_empty() {
                    DoctorObservation::NotApplicable {
                        reason: "command declares no external tools".to_owned(),
                    }
                } else {
                    DoctorObservation::Ready {}
                }
            },
            |tool| tool.observation.clone(),
        );

    DoctorCommand {
        command: contract.path.join(" "),
        observation,
        tools,
    }
}

fn tool_observation(
    tool: &ExternalTool,
    configuration: &Root,
    probe: &impl ExecutableProbe,
) -> DoctorObservation {
    match tool.source {
        DoctorToolSource::CommandOperand => DoctorObservation::InputRequired {
            reason: "the caller supplies this executable at invocation".to_owned(),
        },
        DoctorToolSource::BootstrapEnvironment => match probe.named(tool.name) {
            Ok(()) => DoctorObservation::Ready {},
            Err(reason) => DoctorObservation::Unavailable { reason },
        },
        DoctorToolSource::GatePolicyContract => {
            gate_observation("policy", gate::declared_tools(), probe)
        }
        DoctorToolSource::GateMutationContract => {
            gate_observation("mutation", gate::declared_mutation_tools(), probe)
        }
        DoctorToolSource::ReferenceTransactionContract => match probe.named(tool.name) {
            Ok(()) => DoctorObservation::Ready {},
            Err(reason) => DoctorObservation::Unavailable { reason },
        },
        DoctorToolSource::ProofRuntime => configuration.proof().container().map_or_else(
            || DoctorObservation::InputRequired {
                reason: "local proof.container must name the sealed Proof runtime".to_owned(),
            },
            |runtime| match probe.exact(runtime.as_path()) {
                Ok(()) => DoctorObservation::Ready {},
                Err(reason) => DoctorObservation::Unavailable {
                    reason: format!("configured Proof runtime is unavailable: {reason}"),
                },
            },
        ),
        DoctorToolSource::ProofBattery => DoctorObservation::Unsupported {
            reason: concat!(
                "container-internal battery programs cannot be inspected without ",
                "starting the sealed Proof environment"
            )
            .to_owned(),
        },
        DoctorToolSource::TaskBoundaryContract => match probe.named(tool.name) {
            Ok(()) => DoctorObservation::Ready {},
            Err(reason) => DoctorObservation::Unavailable { reason },
        },
        DoctorToolSource::AdjudicationContract => match probe.named(tool.name) {
            Ok(()) => DoctorObservation::Ready {},
            Err(reason) => DoctorObservation::Unavailable { reason },
        },
        DoctorToolSource::WorktreeConfiguration => {
            let executable = configuration.rtk().executable();
            match probe.rtk(executable) {
                Ok(()) => DoctorObservation::Ready {},
                Err(reason) => DoctorObservation::Unavailable {
                    reason: format!("configured RTK executable {executable:?} is native: {reason}"),
                },
            }
        }
    }
}

fn gate_observation(
    profile: &str,
    declarations: Result<Vec<gate::ExecutableDeclaration>, gate::GateSetError>,
    probe: &impl ExecutableProbe,
) -> DoctorObservation {
    let declarations = match declarations {
        Ok(declarations) => declarations,
        Err(error) => {
            return DoctorObservation::Unsupported {
                reason: format!("the compiled Gate {profile} tool registry is incoherent: {error}"),
            };
        }
    };
    if declarations.is_empty() {
        return DoctorObservation::Unsupported {
            reason: format!("the compiled Gate {profile} tool registry declares no executable"),
        };
    }
    for declaration in declarations {
        if let Err(reason) = probe.exact(declaration.program()) {
            return DoctorObservation::Unavailable {
                reason: format!(
                    "required Gate tool {} is unavailable: {reason}",
                    declaration.name()
                ),
            };
        }
    }
    DoctorObservation::Ready {}
}

fn configuration_refusal(error: &config::LoadError) -> Refusal {
    Refusal::bare(wire::RefusalDiagnostic::new(
        "doctor::configuration-refused".to_owned(),
        None,
        Some("tracked and local inputs admitted by the configuration registry".to_owned()),
        Some(error.diagnostic()),
    ))
}

trait ExecutableProbe {
    fn named(&self, name: &str) -> Result<(), String>;
    fn exact(&self, path: &Path) -> Result<(), String>;

    fn rtk(&self, executable: &str) -> Result<(), String> {
        if Path::new(executable).is_absolute() {
            self.exact(Path::new(executable))
        } else {
            self.named(executable)
        }
    }
}

struct FilesystemProbe<'a> {
    working: &'a Path,
    search_path: Option<&'a OsStr>,
}

impl<'a> FilesystemProbe<'a> {
    const fn new(working: &'a Path, search_path: Option<&'a OsStr>) -> Self {
        Self {
            working,
            search_path,
        }
    }
}

impl ExecutableProbe for FilesystemProbe<'_> {
    fn named(&self, name: &str) -> Result<(), String> {
        let Some(search_path) = self.search_path else {
            return Err(format!(
                "{name} is not available because the executable search path is absent"
            ));
        };
        for directory in std::env::split_paths(search_path) {
            let directory = if directory.is_absolute() {
                directory
            } else {
                self.working.join(directory)
            };
            for candidate in executable_names(name) {
                if executable(&directory.join(candidate)).is_ok() {
                    return Ok(());
                }
            }
        }
        Err(format!(
            "{name} is not available on the configured executable search path"
        ))
    }

    fn exact(&self, path: &Path) -> Result<(), String> {
        executable(path)
    }

    fn rtk(&self, executable: &str) -> Result<(), String> {
        let health = spice::rtk::probe_health(OsStr::new(executable), self.working);
        if health.active() {
            Ok(())
        } else {
            Err(format!(
                "state={} version={} detail={}",
                health.state().as_str(),
                health.version().unwrap_or("unknown"),
                health.detail()
            ))
        }
    }
}

#[cfg(windows)]
fn executable_names(name: &str) -> Vec<String> {
    ["", ".exe", ".cmd", ".bat"]
        .into_iter()
        .map(|suffix| format!("{name}{suffix}"))
        .collect()
}

#[cfg(not(windows))]
fn executable_names(name: &str) -> Vec<String> {
    vec![name.to_owned()]
}

fn executable(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!("{} is not executable", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use wire::{DoctorToolRequirement, DoctorToolSource};

    use super::*;

    struct Probe {
        named: BTreeMap<String, Result<(), String>>,
        exact: Result<(), String>,
    }

    impl ExecutableProbe for Probe {
        fn named(&self, name: &str) -> Result<(), String> {
            self.named
                .get(name)
                .cloned()
                .unwrap_or_else(|| Err(format!("no fixture for {name}")))
        }

        fn exact(&self, _path: &Path) -> Result<(), String> {
            self.exact.clone()
        }
    }

    fn probe(exact: Result<(), String>) -> Probe {
        Probe {
            named: BTreeMap::from([("rtk".to_owned(), Ok(()))]),
            exact,
        }
    }

    #[test]
    fn broken_and_mixed_extension_declarations_remain_visible_without_loading_them() {
        let mixed = [spice::extension::Declaration {
            namespace: spice::extension::Namespace::Gate,
            name: "mixed",
            provider: "fixture",
            value: spice::extension::Value::Vendor(&spice::extension::CODEX),
        }];
        let report = extension_report(&mixed);
        assert!(matches!(
            report.as_slice(),
            [DoctorExtension {
                name,
                observation: DoctorObservation::Unsupported { reason },
                ..
            }] if name == "registry"
                && reason.contains("spice.gates")
                && reason.contains("spice.vendors")
        ));

        let broken = [spice::extension::Declaration {
            namespace: spice::extension::Namespace::Vendor,
            name: "broken",
            provider: "fixture",
            value: spice::extension::Value::Vendor(&spice::extension::CODEX),
        }];
        let report = extension_report(&broken);
        assert!(matches!(
            report.as_slice(),
            [DoctorExtension {
                name,
                observation: DoctorObservation::Unsupported { reason },
                ..
            }] if name == "registry"
                && reason.contains("broken")
                && reason.contains("codex")
        ));
    }

    fn default_command_report(contract: &cli::Contract, probe: &Probe) -> DoctorCommand {
        command_report(contract, &Root::default(), probe)
    }

    #[test]
    fn every_generated_contract_has_one_deterministic_report_row() {
        let first: Vec<_> = cli::contracts()
            .iter()
            .map(|contract| default_command_report(contract, &probe(Ok(()))))
            .collect();
        let second: Vec<_> = cli::contracts()
            .iter()
            .map(|contract| default_command_report(contract, &probe(Ok(()))))
            .collect();

        assert_eq!(first, second);
        assert_eq!(first.len(), cli::contracts().len());
        assert!(first.iter().all(|row| !row.command.is_empty()));
        assert!(first.iter().any(|row| {
            row.tools.is_empty()
                && matches!(row.observation, DoctorObservation::NotApplicable { .. })
        }));
    }

    #[test]
    fn missing_required_executables_and_dynamic_commands_are_not_ready() {
        let missing = probe(Err("fixture executable is absent".to_owned()));
        for command in cli::contracts().iter().filter(|contract| {
            contract.tools.iter().any(|tool| {
                matches!(
                    tool.source,
                    DoctorToolSource::GatePolicyContract | DoctorToolSource::GateMutationContract
                )
            })
        }) {
            let row = default_command_report(command, &missing);
            assert!(matches!(
                row.observation,
                DoctorObservation::Unavailable { ref reason }
                    if reason.contains("fixture executable is absent")
            ));
            assert!(row.tools.iter().all(|tool| {
                tool.requirement == DoctorToolRequirement::Required
                    && matches!(tool.observation, DoctorObservation::Unavailable { .. })
            }));
        }

        let dynamic = cli::contracts()
            .iter()
            .find(|contract| contract.path == ["run"])
            .expect("run contract exists");
        assert!(matches!(
            default_command_report(dynamic, &missing).observation,
            DoctorObservation::InputRequired { .. }
        ));
    }

    #[test]
    fn unsupported_checks_cannot_be_aggregated_as_ready() {
        for command in cli::contracts().iter().filter(|contract| {
            contract
                .tools
                .iter()
                .any(|tool| tool.source == DoctorToolSource::ProofBattery)
        }) {
            let row = default_command_report(command, &probe(Ok(())));
            assert!(!matches!(row.observation, DoctorObservation::Ready {}));
            assert!(
                row.tools.iter().any(|tool| {
                    matches!(tool.observation, DoctorObservation::Unsupported { .. })
                })
            );
        }
    }

    #[test]
    fn doctor_has_no_process_network_authority_or_filesystem_mutation_surface() {
        let source = ::std::include_str!("doctor.rs");
        for forbidden in [
            concat!("std::", "process"),
            concat!("std::", "net"),
            concat!("journal", "::"),
            concat!("Journal", "::"),
            concat!("Command", "::"),
            concat!("Tcp", "Stream"),
            concat!("Udp", "Socket"),
            concat!("fs::", "write"),
            concat!("Open", "Options"),
            concat!("create", "_dir"),
            concat!("set", "_permissions"),
            concat!("write", "_all"),
        ] {
            assert!(
                !source.contains(forbidden),
                "Doctor gained forbidden side-effect surface {forbidden:?}",
            );
        }
    }

    #[test]
    fn subsystem_diagnosis_has_no_execution_or_mutation_surface() {
        let source = ::std::include_str!("doctor/subsystems.rs");
        for forbidden in [
            concat!("std::", "process"),
            concat!("std::", "net"),
            concat!("journal", "::", "Journal", "::", "open"),
            concat!("Command", "::"),
            concat!("Tcp", "Stream"),
            concat!("Udp", "Socket"),
            concat!("fs::", "write"),
            concat!("Open", "Options"),
            concat!("create", "_dir"),
            concat!("set", "_permissions"),
            concat!("write", "_all"),
            concat!("install", "::", "apply"),
            concat!("install", "::", "unapply"),
        ] {
            assert!(
                !source.contains(forbidden),
                "Doctor subsystem diagnosis gained forbidden side-effect surface {forbidden:?}",
            );
        }
    }
}
