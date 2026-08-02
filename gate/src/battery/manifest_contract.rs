use serde::{Deserialize, Serialize};

use super::{ArmFamily, BatteryError, MigrationRefusal};
use crate::core::core_declarations;
use crate::hygiene::HYGIENE_ARMS;
use crate::manifest::{GateId, PREDECESSOR_GATE_RELEASE};
use crate::metrics::STRUCTURAL_ARMS;
use crate::toolchain::TOOLCHAIN_ARMS;

/// The only battery-report and canonical-manifest schema.
pub const GATE_BATTERY_VERSION: u64 = 1;
/// The exact canonical disposition manifest compiled into this crate.
pub const CANONICAL_BATTERY_MANIFEST: &str = include_str!("manifest.toml");

/// Exactly one disposition for one predecessor arm.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case", tag = "disposition")]
pub enum ArmDisposition {
    Compiled {
        gate_id: GateId,
        family: ArmFamily,
        declaration: String,
    },
    MigrationRefusal {
        source_field: Vec<String>,
        owning_release: String,
        reason: String,
        recovery: String,
    },
}

/// One ordered canonical manifest row.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BatteryManifestArm {
    pub(super) predecessor: String,
    #[serde(flatten)]
    pub(super) disposition: ArmDisposition,
}

impl BatteryManifestArm {
    /// Exact predecessor standard-arm spelling.
    #[must_use]
    pub fn predecessor(&self) -> &str {
        &self.predecessor
    }

    /// Compiled declaration or exact migration refusal.
    #[must_use]
    pub const fn disposition(&self) -> &ArmDisposition {
        &self.disposition
    }
}

/// The admitted current disposition manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryManifest {
    version: u64,
    predecessor_release: String,
    pub(super) arms: Vec<BatteryManifestArm>,
}

impl BatteryManifest {
    /// Parse and validate the exact canonical manifest compiled into the crate.
    pub fn canonical() -> Result<Self, BatteryError> {
        Self::decode(CANONICAL_BATTERY_MANIFEST)
    }

    /// Parse a candidate strictly against the closed 25-arm contract.
    pub fn decode(source: &str) -> Result<Self, BatteryError> {
        let manifest: Self = toml::from_str(source)
            .map_err(|error| BatteryError::new(format!("invalid battery manifest: {error}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Current schema version.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Release whose standard battery supplied the row names and order.
    #[must_use]
    pub fn predecessor_release(&self) -> &str {
        &self.predecessor_release
    }

    /// Exactly 25 rows in predecessor order.
    #[must_use]
    pub fn arms(&self) -> &[BatteryManifestArm] {
        &self.arms
    }

    fn validate(&self) -> Result<(), BatteryError> {
        if self.version != GATE_BATTERY_VERSION {
            return Err(BatteryError::new(format!(
                "unsupported battery manifest version {}; expected {GATE_BATTERY_VERSION}",
                self.version
            )));
        }
        if self.predecessor_release != PREDECESSOR_GATE_RELEASE {
            return Err(BatteryError::new(format!(
                "battery predecessor release must be {PREDECESSOR_GATE_RELEASE:?}"
            )));
        }
        if self.arms.len() != GateId::ALL.len() {
            return Err(BatteryError::new(format!(
                "battery manifest must contain exactly {} arms, received {}",
                GateId::ALL.len(),
                self.arms.len()
            )));
        }
        for (position, (arm, expected)) in self.arms.iter().zip(GateId::ALL).enumerate() {
            validate_arm(position, arm, expected)?;
        }
        Ok(())
    }
}

fn validate_arm(
    position: usize,
    arm: &BatteryManifestArm,
    expected: GateId,
) -> Result<(), BatteryError> {
    if arm.predecessor != expected.predecessor_name() {
        return Err(BatteryError::new(format!(
            "battery arm {position} must be predecessor {:?}, received {:?}",
            expected.predecessor_name(),
            arm.predecessor
        )));
    }
    match &arm.disposition {
        ArmDisposition::Compiled {
            gate_id,
            family,
            declaration,
        } => {
            let (expected_family, expected_declaration) = declaration_identity(expected)?;
            if *gate_id != expected {
                return Err(BatteryError::new(format!(
                    "battery arm {position} must compile as {expected}, received {gate_id}"
                )));
            }
            if *family != expected_family || declaration != &expected_declaration {
                return Err(BatteryError::new(format!(
                    "battery arm {position} declaration must be \
                     {expected_family:?} {expected_declaration:?}"
                )));
            }
            Ok(())
        }
        ArmDisposition::MigrationRefusal {
            source_field,
            owning_release,
            reason,
            recovery,
        } => MigrationRefusal {
            source_field: source_field.clone(),
            owning_release: owning_release.clone(),
            reason: reason.clone(),
            recovery: recovery.clone(),
        }
        .validate(position),
    }
}

pub(super) fn declaration_identity(id: GateId) -> Result<(ArmFamily, String), BatteryError> {
    let mut declarations = Vec::new();
    if let Some(declaration) = core_declarations()
        .iter()
        .copied()
        .find(|declaration| declaration.id() == id)
    {
        declarations.push((
            ArmFamily::Core,
            format!("gate::core::{id}::{}", declaration.schema()),
        ));
    }
    if TOOLCHAIN_ARMS
        .into_iter()
        .any(|declaration| declaration.id() == id)
    {
        declarations.push((
            ArmFamily::Toolchain,
            format!("gate::toolchain::{id}::toolchain-v1"),
        ));
    }
    if let Some(declaration) = STRUCTURAL_ARMS
        .into_iter()
        .find(|declaration| declaration.id() == id)
    {
        declarations.push((
            ArmFamily::Metrics,
            format!(
                "gate::metrics::{id}::{}",
                declaration.parser().id().as_str()
            ),
        ));
    }
    if let Some(declaration) = HYGIENE_ARMS
        .into_iter()
        .find(|declaration| declaration.id() == id)
    {
        declarations.push((
            ArmFamily::Hygiene,
            declaration.native_component().to_owned(),
        ));
    }
    match declarations.as_slice() {
        [declaration] => Ok(declaration.clone()),
        [] => Err(BatteryError::new(format!(
            "{id} has no compiled declaration family"
        ))),
        _ => Err(BatteryError::new(format!(
            "{id} is claimed by multiple compiled declaration families"
        ))),
    }
}
