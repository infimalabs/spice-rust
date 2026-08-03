//! Acquisition of one complete hygiene-family analysis.

use std::collections::BTreeSet;
use std::path::Path;

use wire::Digest;

use super::{
    HYGIENE_ARMS, HygieneAnalysisError, HygieneConfiguration, HygieneReport, analyze_hygiene,
};
use crate::input::Contract;
use crate::jurisdiction::{Ignore, Jurisdiction, Rule};
use crate::tree::SelectedTree;

/// Declaration metadata that names tracked policy rather than a path pattern.
const TRACKED_EXCLUSION_SOURCE: &str = "spice.toml:policy.exclude";

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
        let ignore = hygiene_ignore(contract)?;
        let jurisdiction = Jurisdiction::walk(tree.root(), &ignore).map_err(|error| {
            HygieneAnalysisError::Source(format!("hygiene source jurisdiction: {error}"))
        })?;
        let digest = jurisdiction.tree_digest().map_err(|error| {
            HygieneAnalysisError::Source(format!("hygiene source tree identity: {error}"))
        })?;
        Ok(Self {
            root: tree.root(),
            jurisdiction,
            digest,
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
    analyze_hygiene(source.jurisdiction(), configuration)
}

fn hygiene_ignore(contract: &Contract) -> Result<Ignore, HygieneAnalysisError> {
    let mut rules = contract.policy().ignore().rules().to_vec();
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
        }
    }
    Ok(Ignore::from_rules(rules))
}
