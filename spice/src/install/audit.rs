use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::Digest;

use super::record::read_record;
use super::{
    Asset, BINARY_PATH, EXECUTABLE_MODE, GIT_COMMIT_MESSAGE_PATH, GIT_PRE_COMMIT_PATH,
    GIT_REFERENCE_TRANSACTION_PATH, GeneratedImage, InstallError, InstallStatus, Repository,
    installed_binary, read_binary, verify_progress,
};

/// Deterministic relationship between one repository and its installed appliance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApplianceState {
    Current,
    MissingReceipt,
    StaleReceipt,
    MissingBinary,
    BinaryTampered,
    HookTampered,
    AssetTampered,
    AnotherBuild,
}

impl ApplianceState {
    /// Stable diagnostic spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::MissingReceipt => "missing-receipt",
            Self::StaleReceipt => "stale-receipt",
            Self::MissingBinary => "missing-binary",
            Self::BinaryTampered => "binary-tampered",
            Self::HookTampered => "hook-tampered",
            Self::AssetTampered => "asset-tampered",
            Self::AnotherBuild => "another-build",
        }
    }
}

/// Side-effect-free installed-appliance freshness evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplianceAudit {
    state: ApplianceState,
    detail: String,
    repair: Option<String>,
    receipt: Option<Digest>,
    installed_binary: PathBuf,
    current_binary: PathBuf,
}

impl ApplianceAudit {
    /// Classified appliance state.
    #[must_use]
    pub const fn state(&self) -> ApplianceState {
        self.state
    }

    /// Whether the installed bytes and receipt match the current build exactly.
    #[must_use]
    pub const fn is_current(&self) -> bool {
        matches!(self.state, ApplianceState::Current)
    }

    /// Exact diagnostic evidence.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// Current-build command that previews the digest-bound repair.
    #[must_use]
    pub fn repair(&self) -> Option<&str> {
        self.repair.as_deref()
    }

    /// Installed plan identity, when one valid receipt supplied it.
    #[must_use]
    pub const fn receipt(&self) -> Option<&Digest> {
        self.receipt.as_ref()
    }

    /// Receipt-owned installed executable path.
    #[must_use]
    pub fn installed_binary(&self) -> &Path {
        &self.installed_binary
    }

    /// Freshly built executable used as the comparison reference.
    #[must_use]
    pub fn current_binary(&self) -> &Path {
        &self.current_binary
    }
}

/// Audit the receipt-owned appliance against one freshly built Spice executable.
///
/// This performs no mutation and launches no subprocess. Callers that need a
/// current-source answer must build `current_binary` before entering this audit;
/// the reference-transaction hook never calls it.
pub fn audit_appliance(
    repository: &Repository,
    current_binary: &Path,
) -> Result<ApplianceAudit, InstallError> {
    let installed = installed_binary(repository);
    let preview = preview_command(current_binary);
    let loaded = match read_record(repository) {
        Ok(Some(loaded)) => loaded,
        Ok(None) => {
            return Ok(report(
                ApplianceState::MissingReceipt,
                "no installed-appliance receipt exists".to_owned(),
                Some(preview),
                None,
                installed,
                current_binary,
            ));
        }
        Err(error) if stale_receipt(&error) => {
            return Ok(report(
                ApplianceState::StaleReceipt,
                error.to_string(),
                Some(preview),
                None,
                installed,
                current_binary,
            ));
        }
        Err(error) => return Err(error),
    };
    let record = loaded.record;
    let receipt = record.plan_digest.clone();
    if record.status != InstallStatus::Complete {
        return Ok(report(
            ApplianceState::StaleReceipt,
            format!("installed-appliance receipt is {:?}", record.status),
            Some(preview),
            Some(receipt),
            installed,
            current_binary,
        ));
    }
    if let Err(error) = verify_progress(repository, &record) {
        if let InstallError::Drift { path, .. } = &error {
            let state = drift_state(repository, path)?;
            return Ok(report(
                state,
                error.to_string(),
                Some(preview),
                Some(receipt),
                installed,
                current_binary,
            ));
        }
        if stale_receipt(&error) {
            return Ok(report(
                ApplianceState::StaleReceipt,
                error.to_string(),
                Some(preview),
                Some(receipt),
                installed,
                current_binary,
            ));
        }
        return Err(error);
    }
    let Some(binary) = record
        .plan
        .operations
        .iter()
        .find(|operation| operation.asset == Asset::Binary)
    else {
        return Ok(report(
            ApplianceState::MissingBinary,
            "installed-appliance receipt owns no Spice binary".to_owned(),
            Some(preview),
            Some(receipt),
            installed,
            current_binary,
        ));
    };
    let current = GeneratedImage::new(EXECUTABLE_MODE, &read_binary(current_binary)?);
    if current != binary.generated {
        return Ok(report(
            ApplianceState::AnotherBuild,
            format!(
                "installed binary digest {} differs from current build digest {}",
                binary.generated.digest, current.digest
            ),
            Some(preview),
            Some(receipt),
            installed,
            current_binary,
        ));
    }
    Ok(report(
        ApplianceState::Current,
        format!("installed appliance matches receipt {receipt} and the current build"),
        None,
        Some(receipt),
        installed,
        current_binary,
    ))
}

fn stale_receipt(error: &InstallError) -> bool {
    matches!(
        error,
        InstallError::Decode(_)
            | InstallError::InvalidRecord(_)
            | InstallError::LegacyRecord { .. }
            | InstallError::PathShape(_)
            | InstallError::PathOutsideRoot(_)
            | InstallError::ParentChanged(_)
    )
}

fn drift_state(repository: &Repository, path: &str) -> Result<ApplianceState, InstallError> {
    if path == BINARY_PATH {
        return match fs::symlink_metadata(installed_binary(repository)) {
            Ok(_) => Ok(ApplianceState::BinaryTampered),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Ok(ApplianceState::MissingBinary)
            }
            Err(source) => Err(InstallError::io(
                "inspect installed Spice executable",
                installed_binary(repository),
                source,
            )),
        };
    }
    if matches!(
        path,
        GIT_PRE_COMMIT_PATH | GIT_COMMIT_MESSAGE_PATH | GIT_REFERENCE_TRANSACTION_PATH
    ) {
        Ok(ApplianceState::HookTampered)
    } else {
        Ok(ApplianceState::AssetTampered)
    }
}

fn preview_command(current_binary: &Path) -> String {
    current_binary.to_str().map_or_else(
        || format!("run {current_binary:?} with the `install` argument"),
        |binary| shell_words::join([binary, "install"]),
    )
}

fn report(
    state: ApplianceState,
    detail: String,
    repair: Option<String>,
    receipt: Option<Digest>,
    installed_binary: PathBuf,
    current_binary: &Path,
) -> ApplianceAudit {
    ApplianceAudit {
        state,
        detail,
        repair,
        receipt,
        installed_binary,
        current_binary: current_binary.to_path_buf(),
    }
}
