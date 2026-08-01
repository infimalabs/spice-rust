//! Static external-tool posture carried by generated command contracts.

#[cfg(test)]
use std::collections::BTreeSet;

#[cfg(test)]
use super::{Contract, Guard};
pub(crate) use wire::{
    DoctorToolBounds as ToolBounds, DoctorToolCancellation as ToolCancellation,
    DoctorToolMode as ToolMode, DoctorToolNetwork as ToolNetwork,
    DoctorToolRequirement as ToolRequirement, DoctorToolSource as ToolSource,
};

/// One external tool declaration carried by the closed command registry.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ExternalTool {
    pub(crate) name: &'static str,
    pub(crate) owner: &'static str,
    pub(crate) mode: ToolMode,
    pub(crate) requirement: ToolRequirement,
    pub(crate) source: ToolSource,
    pub(crate) bounds: ToolBounds,
    pub(crate) cancellation: ToolCancellation,
    pub(crate) network: ToolNetwork,
}

pub(super) const NO_TOOLS: &[ExternalTool] = &[];

pub(super) const DOCTOR_TOOLS: &[ExternalTool] = &[ExternalTool {
    name: "rtk",
    owner: "rtk",
    mode: ToolMode::Observe,
    requirement: ToolRequirement::Optional,
    source: ToolSource::WorktreeConfiguration,
    bounds: ToolBounds::NotExecuted,
    cancellation: ToolCancellation::NotExecuted,
    network: ToolNetwork::NotExecuted,
}];

pub(super) const CALLER_COMMAND: &[ExternalTool] = &[ExternalTool {
    name: "CMD",
    owner: "caller",
    mode: ToolMode::Launch,
    requirement: ToolRequirement::Required,
    source: ToolSource::CommandOperand,
    bounds: ToolBounds::ChildOwned,
    cancellation: ToolCancellation::ChildOwned,
    network: ToolNetwork::ChildOwned,
}];

pub(super) const GATE_POLICY_TOOLS: &[ExternalTool] = &[ExternalTool {
    name: "gate policy executable",
    owner: "gate::Contract",
    mode: ToolMode::Launch,
    requirement: ToolRequirement::Required,
    source: ToolSource::GateContract,
    bounds: ToolBounds::GateContract,
    cancellation: ToolCancellation::Deadline,
    network: ToolNetwork::Permitted,
}];

pub(super) const GATE_MUTATION_TOOLS: &[ExternalTool] = &[ExternalTool {
    name: "gate mutation executable",
    owner: "gate::kill",
    mode: ToolMode::Launch,
    requirement: ToolRequirement::Required,
    source: ToolSource::GateContract,
    bounds: ToolBounds::GateContract,
    cancellation: ToolCancellation::Deadline,
    network: ToolNetwork::Permitted,
}];

pub(super) const PROOF_TOOLS: &[ExternalTool] = &[
    ExternalTool {
        name: "OCI runtime",
        owner: "proof::OciRuntime",
        mode: ToolMode::Launch,
        requirement: ToolRequirement::Required,
        source: ToolSource::ProofRuntime,
        bounds: ToolBounds::ProofLimits,
        cancellation: ToolCancellation::Deadline,
        network: ToolNetwork::Denied,
    },
    ExternalTool {
        name: "sealed battery member",
        owner: "proof::Battery",
        mode: ToolMode::Launch,
        requirement: ToolRequirement::Required,
        source: ToolSource::ProofBattery,
        bounds: ToolBounds::ProofLimits,
        cancellation: ToolCancellation::Deadline,
        network: ToolNetwork::Denied,
    },
];

#[cfg(test)]
pub(super) fn posture_violations(contracts: &[Contract]) -> Vec<String> {
    let mut violations = Vec::new();
    for contract in contracts {
        let path = contract.path.join(" ");
        let launches = contract
            .tools
            .iter()
            .any(|tool| tool.mode == ToolMode::Launch);
        if launches != contract.guards.contains(&Guard::Toolful) {
            violations.push(format!(
                "{path}: toolful guard and launch declarations disagree"
            ));
        }

        let mut declarations = BTreeSet::new();
        for tool in contract.tools {
            if tool.name.trim().is_empty() {
                violations.push(format!("{path}: external tool has no name"));
            }
            if tool.owner.trim().is_empty() {
                violations.push(format!(
                    "{path}: external tool {:?} has no owner",
                    tool.name
                ));
            }
            if !declarations.insert(tool.name) {
                violations.push(format!(
                    "{path}: external tool {:?} is declared more than once",
                    tool.name
                ));
            }

            let execution = (tool.bounds, tool.cancellation, tool.network);
            match tool.mode {
                ToolMode::Launch => {
                    if matches!(tool.bounds, ToolBounds::NotExecuted)
                        || matches!(tool.cancellation, ToolCancellation::NotExecuted)
                        || matches!(tool.network, ToolNetwork::NotExecuted)
                    {
                        violations.push(format!(
                            "{path}: launched tool {:?} has an observation-only posture",
                            tool.name
                        ));
                    }
                }
                ToolMode::Observe => {
                    let not_executed = (
                        ToolBounds::NotExecuted,
                        ToolCancellation::NotExecuted,
                        ToolNetwork::NotExecuted,
                    );
                    if execution != not_executed {
                        violations.push(format!(
                            "{path}: observed tool {:?} carries execution posture",
                            tool.name
                        ));
                    }
                }
            }
            match tool.source {
                ToolSource::CommandOperand => {
                    let expected = (
                        ToolMode::Launch,
                        ToolBounds::ChildOwned,
                        ToolCancellation::ChildOwned,
                        ToolNetwork::ChildOwned,
                    );
                    if (tool.mode, tool.bounds, tool.cancellation, tool.network) != expected {
                        violations.push(format!(
                            "{path}: command operand {:?} does not delegate its execution posture",
                            tool.name
                        ));
                    }
                }
                ToolSource::GateContract => {
                    let expected = (
                        ToolMode::Launch,
                        ToolBounds::GateContract,
                        ToolCancellation::Deadline,
                        ToolNetwork::Permitted,
                    );
                    if (tool.mode, tool.bounds, tool.cancellation, tool.network) != expected {
                        violations.push(format!(
                            "{path}: Gate tool {:?} escaped the Gate contract",
                            tool.name
                        ));
                    }
                }
                ToolSource::ProofRuntime | ToolSource::ProofBattery => {
                    let expected = (
                        ToolMode::Launch,
                        ToolBounds::ProofLimits,
                        ToolCancellation::Deadline,
                        ToolNetwork::Denied,
                    );
                    if (tool.mode, tool.bounds, tool.cancellation, tool.network) != expected {
                        violations.push(format!(
                            "{path}: Proof tool {:?} escaped the sealed limits",
                            tool.name
                        ));
                    }
                }
                ToolSource::WorktreeConfiguration => {
                    if tool.mode != ToolMode::Observe
                        || tool.requirement != ToolRequirement::Optional
                    {
                        violations.push(format!(
                            "{path}: worktree tool {:?} is not an optional observation",
                            tool.name
                        ));
                    }
                }
            }
        }
    }
    violations
}
