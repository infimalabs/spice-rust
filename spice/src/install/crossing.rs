//! The one forward crossing from install schema 18.
//!
//! Schema 18 published the browser app as receipt-owned sidecar files under
//! `.spice-install/app`: a current-generation pointer and every retained
//! generation's files. Schema 19 publishes only the executable that links the
//! app. A complete, exact schema-18 receipt crosses forward once: its app
//! operations become evidence of the sidecar files Spice owns, every other
//! effect keeps its reversal authority in a completed schema-19 record, and
//! the refresh derived from that record discards each sidecar file only while
//! it still holds the image schema 18 recorded. Nothing else reads schema 18.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use super::path::relative_path;
use super::record::InstallRecord;
use super::{
    Asset, BoundDestination, DATA_MODE, GeneratedImage, Generation, HookActivationPlan,
    InstallError, InstallPlan, InstallStatus, Location, Operation, PLAN_VERSION, ParentObservation,
    Repository, RepositoryIdentity, Retirement, SymbolProvenance, bind_destination,
};

/// The retained schema this module alone reads.
pub(super) const SCHEMA: u16 = 18;
/// The task that owns the crossing and every refusal of a schema-18 receipt.
pub(super) const OWNER: &str = "CANVAS-dWF0m0T7z";

/// Directory every sidecar discard anchors at.
const ANCHOR: &str = ".spice-install";
/// Directory schema 18 published the app beneath.
const SIDECAR_ROOT: &str = ".spice-install/app";
/// Schema-18 pointer naming the current app generation.
const POINTER: &str = ".spice-install/app/current";
/// Schema-18 directory holding each published generation's files.
const GENERATIONS: &str = ".spice-install/app/generations";

/// One receipt-owned sidecar file and the exact image schema 18 recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Sidecar {
    path: String,
    image: GeneratedImage,
}

/// A schema-18 receipt carried forward into schema 19.
pub(super) struct Crossed {
    /// The completed schema-19 record of every effect the receipt keeps.
    pub(super) record: InstallRecord,
    /// The schema-18 plan digest, the generation the receipt was published as.
    pub(super) original_digest: Digest,
    /// Every sidecar file the receipt owned, in discard order.
    pub(super) sidecars: Vec<Sidecar>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedRecord {
    version: u16,
    repository: RepositoryIdentity,
    plan: RetainedPlan,
    plan_digest: Digest,
    destinations: Vec<(Location, String)>,
    status: InstallStatus,
    completed: usize,
    installed: Vec<Option<Generation>>,
    installed_parents: Vec<Option<Generation>>,
    restored: Vec<Option<Generation>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retired_parents: Vec<Option<Generation>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    restored_retirements: Vec<Option<Generation>>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedPlan {
    version: u16,
    repository: RepositoryIdentity,
    record: BoundDestination,
    operations: Vec<RetainedOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app: Option<Digest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    app_predecessors: Vec<Digest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retirements: Vec<Retirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hook_activation: Option<HookActivationPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    symbols: Option<SymbolProvenance>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedOperation {
    asset: RetainedAsset,
    location: Location,
    path: String,
    destination: String,
    parent: ParentObservation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<GeneratedImage>,
    generated: GeneratedImage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    superseded: Option<GeneratedImage>,
    introduced_parents: Vec<String>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(untagged)]
enum RetainedAsset {
    Current(Asset),
    Sidecar(SidecarAsset),
}

/// The per-file app assets schema 18 published.
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SidecarAsset {
    AppModule,
    AppWasm,
    AppHtml,
    AppChrome,
    AppComposer,
    AppMosaic,
    AppResponsive,
    AppSpiceSidebar,
    AppStyle,
    AppWork,
    AppRenderer(wire::manifest::renderer::File),
    AppCurrent,
}

impl SidecarAsset {
    /// The file name this asset carried inside its generation directory.
    fn file(self) -> Option<String> {
        let name = match self {
            Self::AppModule => "app.module",
            Self::AppWasm => "app_bg.wasm",
            Self::AppHtml => "app.html",
            Self::AppChrome => "chrome.css",
            Self::AppComposer => "composer.css",
            Self::AppMosaic => "mosaic.css",
            Self::AppResponsive => "responsive.css",
            Self::AppSpiceSidebar => "spice_sidebar.css",
            Self::AppStyle => "style.css",
            Self::AppWork => "work.css",
            Self::AppRenderer(file) => return Some(file.to_string()),
            Self::AppCurrent => return None,
        };
        Some(name.to_owned())
    }
}

/// Admit exactly one complete, strictly verified schema-18 receipt.
///
/// The receipt must be canonical, its digest must name its plan, its progress
/// must be exactly complete, and its app operations must name exactly the
/// schema-18 sidecar layout for its current and retained generations.
/// Anything else refuses as [`InstallError::RetainedRecord`].
pub(super) fn cross(record_path: &Path, bytes: &[u8]) -> Result<Crossed, InstallError> {
    let refuse = |reason: String| InstallError::RetainedRecord {
        path: record_path.to_path_buf(),
        owner: OWNER,
        reason,
    };
    let retained: RetainedRecord =
        wire::decode(bytes).map_err(|error| refuse(format!("it does not decode: {error}")))?;
    if wire::encode(&retained).map_err(InstallError::Encode)? != bytes {
        return Err(refuse("it is not canonical".to_owned()));
    }
    if retained.version != SCHEMA || retained.plan.version != SCHEMA {
        return Err(refuse(
            "its record and plan do not both name schema 18".to_owned(),
        ));
    }
    if retained.status != InstallStatus::Complete {
        return Err(refuse(format!(
            "it is {:?}; finish or unapply it with the schema-18 release first",
            retained.status
        )));
    }
    let plan_bytes = wire::encode(&retained.plan).map_err(InstallError::Encode)?;
    if Digest::compute(Domain::InstallPlan, &plan_bytes) != retained.plan_digest
        || retained.repository != retained.plan.repository
    {
        return Err(refuse(
            "its digest and repository do not close over its plan".to_owned(),
        ));
    }
    let plan = &retained.plan;
    let operations = plan.operations.len();
    let retirements = plan.retirements.len();
    if retained.completed != operations + retirements + usize::from(plan.hook_activation.is_some())
        || retained.installed.len() != operations
        || retained.installed_parents.len() != operations
        || retained.restored.len() != operations
        || retained.retired_parents.len() != retirements
        || retained.restored_retirements.len() != retirements
        || retained.installed.iter().any(Option::is_none)
        || retained.installed_parents.iter().any(Option::is_none)
        || !plan
            .operations
            .iter()
            .map(|operation| (operation.location, operation.destination.as_str()))
            .eq(retained
                .destinations
                .iter()
                .map(|(location, path)| (*location, path.as_str())))
    {
        return Err(refuse("its progress is not exactly complete".to_owned()));
    }
    let sidecars = sidecars(&retained.plan).map_err(|reason| refuse(reason.to_owned()))?;
    let original_digest = retained.plan_digest.clone();
    let record = keep(retained)?;
    Ok(Crossed {
        record,
        original_digest,
        sidecars,
    })
}

/// The exact sidecar evidence of one schema-18 plan, in discard order.
fn sidecars(plan: &RetainedPlan) -> Result<Vec<Sidecar>, &'static str> {
    let published = plan
        .app
        .iter()
        .chain(&plan.app_predecessors)
        .collect::<BTreeSet<_>>();
    if published.len() != plan.app_predecessors.len() + usize::from(plan.app.is_some()) {
        return Err("its app generations repeat");
    }
    let mut paths = BTreeSet::new();
    let mut generations = BTreeSet::new();
    let mut sidecars = Vec::new();
    for operation in &plan.operations {
        let RetainedAsset::Sidecar(asset) = operation.asset else {
            continue;
        };
        if operation.location != Location::CommonGit || operation.generated.mode != DATA_MODE {
            return Err("an app operation does not match its schema-18 contract");
        }
        match asset.file() {
            None => {
                let pointer = plan.app.as_ref().map(|current| {
                    GeneratedImage::new(DATA_MODE, format!("{current}\n").as_bytes())
                });
                if operation.path != POINTER || pointer.as_ref() != Some(&operation.generated) {
                    return Err("its app pointer does not name its current generation");
                }
            }
            Some(file) => {
                let generation = operation
                    .path
                    .strip_prefix(GENERATIONS)
                    .and_then(|path| path.strip_prefix('/'))
                    .and_then(|path| path.strip_suffix(file.as_str()))
                    .and_then(|path| path.strip_suffix('/'))
                    .and_then(|generation| generation.parse::<Digest>().ok())
                    .filter(|generation| published.contains(generation))
                    .ok_or("an app file does not lie in a published generation")?;
                generations.insert(generation);
            }
        }
        if !paths.insert(operation.path.as_str()) {
            return Err("an app path repeats");
        }
        sidecars.push(Sidecar {
            path: operation.path.clone(),
            image: operation.generated.clone(),
        });
    }
    let pointed = paths.contains(POINTER);
    if pointed != plan.app.is_some() || !generations.iter().eq(published.iter().copied()) {
        return Err("its app operations do not publish exactly its app generations");
    }
    sidecars.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(sidecars)
}

/// The completed schema-19 record of every effect a schema-18 receipt keeps:
/// its fixed, executable, symbol, and hook operations with their reversal
/// images, its fixed retirements, and its hook activation. Retirements of
/// app generations schema 18 already discarded are complete history.
fn keep(retained: RetainedRecord) -> Result<InstallRecord, InstallError> {
    let RetainedRecord {
        plan,
        installed,
        installed_parents,
        restored,
        retired_parents,
        restored_retirements,
        ..
    } = retained;
    let mut operations = Vec::new();
    let mut kept_installed = Vec::new();
    let mut kept_parents = Vec::new();
    let mut kept_restored = Vec::new();
    for (((operation, installed), parent), restored) in plan
        .operations
        .into_iter()
        .zip(installed)
        .zip(installed_parents)
        .zip(restored)
    {
        let RetainedAsset::Current(asset) = operation.asset else {
            continue;
        };
        operations.push(Operation {
            asset,
            location: operation.location,
            path: operation.path,
            destination: operation.destination,
            parent: operation.parent,
            previous: operation.previous,
            generated: operation.generated,
            superseded: operation.superseded,
            publication_parent: None,
            introduced_parents: operation.introduced_parents,
        });
        kept_installed.push(installed);
        kept_parents.push(parent);
        kept_restored.push(restored);
    }
    let mut retirements = Vec::new();
    let mut kept_retired = Vec::new();
    let mut kept_restored_retirements = Vec::new();
    for ((retirement, parent), restored) in plan
        .retirements
        .into_iter()
        .zip(retired_parents)
        .zip(restored_retirements)
    {
        if retirement.path.starts_with(SIDECAR_ROOT) {
            continue;
        }
        retirements.push(retirement);
        kept_retired.push(parent);
        kept_restored_retirements.push(restored);
    }
    let plan = InstallPlan {
        version: PLAN_VERSION,
        repository: plan.repository,
        record: plan.record,
        operations,
        app: plan.app,
        retirements,
        hook_activation: plan.hook_activation,
        symbols: plan.symbols,
    };
    InstallRecord::crossed(
        plan,
        (kept_installed, kept_parents, kept_restored),
        (kept_retired, kept_restored_retirements),
    )
}

/// Discard every receipt-owned sidecar file while it still holds the image
/// schema 18 recorded: the pointer first, then each generation's files.
pub(super) fn retirements(
    repository: &Repository,
    sidecars: &[Sidecar],
) -> Result<Vec<Retirement>, InstallError> {
    sidecars
        .iter()
        .map(|sidecar| {
            let (destination, parent) = binding(repository, &sidecar.path)?;
            Ok(Retirement {
                path: sidecar.path.clone(),
                destination,
                parent,
                previous: None,
                observed: None,
                superseded: Some(sidecar.image.clone()),
                publication_parent: None,
                introduced_parents: Vec::new(),
                discard: true,
            })
        })
        .collect()
}

/// Whether `path` names a schema-18 sidecar file.
pub(super) fn is_sidecar(path: &str) -> bool {
    path == POINTER
        || path
            .strip_prefix(GENERATIONS)
            .is_some_and(|path| path.starts_with('/'))
}

/// The binding a sidecar discard carries.
///
/// It anchors at `.spice-install` and names every directory between that
/// anchor and the file as the suffix its discard run withdraws: innermost
/// first, each only while empty. The binding therefore stays valid after any
/// of those directories is withdrawn, and the run that empties the last
/// generation also withdraws `generations` and `app`.
pub(super) fn binding(
    repository: &Repository,
    path: &str,
) -> Result<(String, ParentObservation), InstallError> {
    let anchor = bind_destination(repository.common_dir(), SIDECAR_ROOT)?;
    if !anchor.parent.missing.is_empty() {
        return Err(InstallError::ParentChanged(
            repository.common_dir().join(ANCHOR),
        ));
    }
    let below = below_anchor(path).ok_or_else(|| {
        InstallError::InvalidRecord("sidecar path lies outside its anchor".to_owned())
    })?;
    let missing = below
        .parent()
        .map(relative_path)
        .transpose()?
        .unwrap_or_default();
    let destination = relative_path(&Path::new(&anchor.parent.ancestor).join(below))?;
    Ok((
        destination,
        ParentObservation {
            ancestor: anchor.parent.ancestor,
            generation: anchor.parent.generation,
            missing,
        },
    ))
}

/// `path` below the sidecar anchor, rebuilt from its components so it is
/// spelled as `bind_destination` spells the same file on this platform.
fn below_anchor(path: &str) -> Option<PathBuf> {
    Path::new(path)
        .strip_prefix(ANCHOR)
        .ok()
        .map(|below| below.components().collect())
}

/// Sidecar discards close the retirement inventory: exact, witnessed discards
/// in strict path order, each naming its own directory suffix.
pub(super) fn validate_retirements(retirements: &[Retirement]) -> Result<(), InstallError> {
    let mut prior: Option<&str> = None;
    for retirement in retirements {
        let suffix = below_anchor(&retirement.path)
            .as_deref()
            .and_then(Path::parent)
            .and_then(|suffix| relative_path(suffix).ok());
        let generation_file = retirement
            .path
            .strip_prefix(GENERATIONS)
            .and_then(|path| path.strip_prefix('/'))
            .and_then(|path| path.split_once('/'))
            .is_some_and(|(generation, file)| {
                generation.parse::<Digest>().is_ok() && !file.is_empty() && !file.contains('/')
            });
        if !(retirement.path == POINTER || generation_file)
            || !retirement.discard
            || retirement.superseded.is_none()
            || suffix.as_deref() != Some(retirement.parent.missing.as_str())
            || prior.is_some_and(|prior| prior >= retirement.path.as_str())
        {
            return Err(InstallError::InvalidRecord(
                "sidecar retirements are not exact, ordered discards".to_owned(),
            ));
        }
        prior = Some(&retirement.path);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
