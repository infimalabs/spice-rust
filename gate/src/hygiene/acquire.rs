//! Acquisition of one complete hygiene-family analysis.

use std::collections::BTreeSet;
use std::path::Path;

use wire::Digest;

use super::analyze::is_python_path;
use super::{
    COMPILED_DOCUMENT, COMPILED_LAYER, CURRENT_RELEASE, HYGIENE_ARMS, HygieneAnalysisError,
    HygieneConfiguration, HygieneMeasurement, HygieneProvenance, HygieneReport, SuppressedPath,
    analyze_hygiene, analyze_hygiene_arm,
};
use crate::GateId;
use crate::input::Contract;
use crate::jurisdiction::{Admission, Ignore, Jurisdiction, Rule, Shape};
use crate::tree::SelectedTree;

/// Declaration metadata that names tracked policy rather than a path pattern.
const TRACKED_EXCLUSION_SOURCE: &str = "spice-rust.toml:policy.exclude";

/// One immutable selected tree and the complete hygiene jurisdiction captured
/// from it.
///
/// The borrow keeps the private [`SelectedTree`] materialization alive for the
/// entire acquisition. Its digest is computed once from the same jurisdiction
/// every analyzer receives.
#[derive(Debug)]
pub struct HygieneSource<'a> {
    root: &'a Path,
    jurisdiction: Jurisdiction,
    digest: Digest,
    suppressed: Vec<SuppressedPath>,
}

impl<'a> HygieneSource<'a> {
    /// Capture the tracked and compiled hygiene jurisdiction over one selected
    /// tree.
    ///
    /// # Errors
    ///
    /// Refuses an invalid compiled exclusion, an incomplete walk, or a source
    /// whose admitted bytes cannot produce one complete identity.
    pub fn capture(
        tree: &'a SelectedTree,
        contract: &Contract,
    ) -> Result<Self, HygieneAnalysisError> {
        let (ignore, provenance) = hygiene_ignore(contract)?;
        let jurisdiction = Jurisdiction::walk(tree.root(), &ignore).map_err(|error| {
            HygieneAnalysisError::Source(format!("hygiene source jurisdiction: {error}"))
        })?;
        let digest = jurisdiction.tree_digest().map_err(|error| {
            HygieneAnalysisError::Source(format!("hygiene source tree identity: {error}"))
        })?;
        let suppressed = suppressed_candidates(&jurisdiction, &ignore, &provenance)?;
        Ok(Self {
            root: tree.root(),
            jurisdiction,
            digest,
            suppressed,
        })
    }

    /// Immutable selected-tree root every analyzer reads.
    #[must_use]
    pub const fn root(&self) -> &Path {
        self.root
    }

    /// Complete path inventory, including every tracked or compiled exclusion.
    #[must_use]
    pub const fn jurisdiction(&self) -> &Jurisdiction {
        &self.jurisdiction
    }

    /// Identity of every admitted source byte supplied to the family.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    /// What this jurisdiction withheld from measurement, each naming its rule.
    ///
    /// The jurisdiction retains a rule *index*, and the rule list it indexes
    /// lives only here, so this is the last point at which a suppression can be
    /// attributed to the layer that asked for it.
    #[must_use]
    pub fn suppressed(&self) -> &[SuppressedPath] {
        &self.suppressed
    }
}

/// Acquire all five hygiene measurements from one captured source.
///
/// This entrypoint performs no executable lookup or process launch. The
/// selected source, tracked configuration, compiled analyzers, and Rust parser
/// are its complete authority.
///
/// # Errors
///
/// Returns a source or analysis refusal that prevents a complete five-arm
/// report.
pub fn acquire_hygiene(
    source: &HygieneSource<'_>,
    configuration: &HygieneConfiguration,
) -> Result<HygieneReport, HygieneAnalysisError> {
    let configuration = configuration
        .clone()
        .with_suppressions(source.suppressed().to_vec())?;
    analyze_hygiene(source.jurisdiction(), &configuration)
}

/// Acquire exactly one hygiene measurement from the shared captured source.
pub(crate) fn acquire_hygiene_arm(
    source: &HygieneSource<'_>,
    configuration: &HygieneConfiguration,
    gate_id: GateId,
) -> Result<HygieneMeasurement, HygieneAnalysisError> {
    let configuration = configuration
        .clone()
        .with_suppressions(source.suppressed().to_vec())?;
    analyze_hygiene_arm(source.jurisdiction(), &configuration, gate_id)
}

/// Acquire one hygiene arm over only its authored path reach.
///
/// The path projection comes from the complete immutable jurisdiction, so it
/// preserves the original ignore decisions without walking or snapshotting an
/// unrelated file. Suppressed ancestors are retained only when they explain a
/// reached descendant.
pub(crate) fn acquire_hygiene_arm_reached(
    source: &HygieneSource<'_>,
    configuration: &HygieneConfiguration,
    gate_id: GateId,
    reached: &[String],
) -> Result<HygieneMeasurement, HygieneAnalysisError> {
    let jurisdiction = source.jurisdiction.select_paths(reached);
    let suppressions: Vec<_> = source
        .suppressed()
        .iter()
        .filter(|suppressed| {
            reached.iter().any(|path| {
                path == &suppressed.path
                    || path
                        .strip_prefix(&suppressed.path)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
        })
        .cloned()
        .collect();
    let configuration = configuration.clone().with_suppressions(suppressions)?;
    analyze_hygiene_arm(&jurisdiction, &configuration, gate_id)
}

/// Everything the walk withheld from measurement, paired with the rule that
/// withheld it.
///
/// Two shapes withhold: an ignored Python file, which is one candidate lost,
/// and an ignored directory, which loses every candidate beneath it at once.
/// The walk records the pruned directory and stops there, so nothing downstream
/// can name those candidates or count them. The directory itself is the whole
/// truth available: a region of the selected tree no analyzer entered, and the
/// rule that closed it. Reporting it is what keeps that loss from being silent.
/// A readmitted path was measured, and an ignored file that is no Python
/// candidate was never going to be, so neither withholds anything.
fn suppressed_candidates(
    jurisdiction: &Jurisdiction,
    ignore: &Ignore,
    provenance: &[HygieneProvenance],
) -> Result<Vec<SuppressedPath>, HygieneAnalysisError> {
    let mut suppressed = Vec::new();
    for entry in jurisdiction.entries() {
        let Admission::Ignored(rule) = entry.admission() else {
            continue;
        };
        let shape = entry.shape();
        match shape {
            Shape::File if is_python_path(entry.path()) => {}
            Shape::Directory => {}
            Shape::File | Shape::Other => continue,
        }
        let Some((rule, provenance)) = ignore.rules().get(rule).zip(provenance.get(rule)) else {
            return Err(HygieneAnalysisError::Source(format!(
                "hygiene walk decided {} by rule {rule} but the composed policy has no such rule",
                entry.path()
            )));
        };
        suppressed.push(SuppressedPath {
            path: entry.path().to_owned(),
            shape: shape.as_str().to_owned(),
            rule: rule.to_string(),
            provenance: provenance.clone(),
        });
    }
    Ok(suppressed)
}

/// Compose the hygiene walk policy and the origin of each rule in it.
///
/// The returned provenance is index-aligned with [`Ignore::rules`], which is
/// what an [`Admission::Ignored`] index means. Three ordered ranges make it up:
/// the compiled walk defaults, the tracked `gate.walk.skip` entries, and the
/// compiled per-arm excludes appended here.
fn hygiene_ignore(
    contract: &Contract,
) -> Result<(Ignore, Vec<HygieneProvenance>), HygieneAnalysisError> {
    let mut rules = contract.policy().ignore().rules().to_vec();
    let mut provenance = walk_provenance(rules.len(), contract.compiled_walk_rules());
    let mut compiled = BTreeSet::new();
    for declaration in HYGIENE_ARMS {
        for exclusion in declaration.jurisdiction().excludes() {
            if *exclusion == TRACKED_EXCLUSION_SOURCE || !compiled.insert(*exclusion) {
                continue;
            }
            let rule = Rule::parse(exclusion)
                .map_err(|error| {
                    HygieneAnalysisError::Source(format!(
                        "compiled hygiene exclusion {exclusion:?}: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    HygieneAnalysisError::Source(format!(
                        "compiled hygiene exclusion {exclusion:?} has no rule"
                    ))
                })?;
            rules.push(rule);
            provenance.push(compiled_arm_provenance());
        }
    }
    Ok((Ignore::from_rules(rules), provenance))
}

/// Attribute each walk rule to the layer that supplied it.
///
/// The first `compiled` rules are the compiled defaults and the rest are
/// tracked, so a tracked rule's own declaration index is its distance past the
/// boundary. This is the whole reason the boundary is carried rather than
/// recomputed: nothing downstream of the composed list can recover it.
fn walk_provenance(rules: usize, compiled: usize) -> Vec<HygieneProvenance> {
    (0..rules)
        .map(|index| {
            index
                .checked_sub(compiled)
                .map_or_else(compiled_walk_provenance, tracked_walk_provenance)
        })
        .collect()
}

fn compiled_walk_provenance() -> HygieneProvenance {
    HygieneProvenance {
        release: CURRENT_RELEASE.to_owned(),
        layer: COMPILED_LAYER.to_owned(),
        document: COMPILED_DOCUMENT.to_owned(),
        field: "gate::input::DEFAULT_SKIP".to_owned(),
    }
}

fn tracked_walk_provenance(index: usize) -> HygieneProvenance {
    HygieneProvenance {
        release: CURRENT_RELEASE.to_owned(),
        layer: "repository".to_owned(),
        document: "spice-rust.toml".to_owned(),
        field: format!("gate.walk.skip[{index}]"),
    }
}

fn compiled_arm_provenance() -> HygieneProvenance {
    HygieneProvenance {
        release: CURRENT_RELEASE.to_owned(),
        layer: COMPILED_LAYER.to_owned(),
        document: COMPILED_DOCUMENT.to_owned(),
        field: "gate::hygiene::HygieneJurisdiction::excludes".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The compiled defaults prune whole directories, and a pruned directory is
    /// a suppression, so a repository that tracks a path under `target/`
    /// reaches the compiled half through a real acquisition;
    /// `gate/tests/hygiene_acquire.rs` does exactly that. What stays here is
    /// the arithmetic over a boundary no fixture has to arrange.
    #[test]
    fn the_boundary_decides_which_layer_owns_each_walk_rule() {
        let provenance = walk_provenance(4, 2);

        assert_eq!(
            provenance
                .iter()
                .map(|item| (item.layer.as_str(), item.field.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("compiled", "gate::input::DEFAULT_SKIP"),
                ("compiled", "gate::input::DEFAULT_SKIP"),
                ("repository", "gate.walk.skip[0]"),
                ("repository", "gate.walk.skip[1]"),
            ]
        );
        assert!(provenance.iter().all(HygieneProvenance::is_complete));
    }

    #[test]
    fn a_policy_of_only_compiled_defaults_attributes_nothing_to_the_repository() {
        assert!(
            walk_provenance(2, 2)
                .iter()
                .all(|item| item.layer == "compiled")
        );
        assert!(walk_provenance(0, 0).is_empty());
    }
}
