use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use crate::repository::Repository;

use super::{Generation, InstallError};

/// One Spice-owned asset in canonical installation order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Asset {
    /// Retired version-5 asset retained only so its record can name its owner.
    Exclusions,
    GitPreCommit,
    GitCommitMessage,
    GitReferenceTransaction,
    HookEntry,
    HookStatic,
    HookZsh,
    HookBash,
    HookWrapperSpice,
    Skill,
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
    Binary,
    /// Retained dSYM `Info.plist` published only with a symbol-bearing executable.
    SymbolsInfo,
    /// Retained dSYM DWARF image sharing the installed executable `LC_UUID`.
    SymbolsDwarf,
}

/// Root against which an operation's relative path resolves.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Location {
    /// Retired: no operation resolves here, and none can.
    ///
    /// An install identifies its repository by common directory alone, which
    /// `RepositoryIdentity::capture` enforces by capturing no worktree at
    /// all. A worktree-relative destination is therefore unresolvable from the
    /// record that owns it: one record serves every linked worktree, so the
    /// root this variant names is whichever worktree happened to run install
    /// rather than a place the record can rediscover later. Current appliance
    /// assets therefore live exclusively below [`Self::CommonGit`].
    Worktree,
    CommonGit,
}

impl Location {
    pub(super) fn root(self, repository: &Repository) -> &Path {
        match self {
            Self::Worktree => repository.worktree_root(),
            Self::CommonGit => repository.common_dir(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RepositoryIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) worktree: Option<PathBuf>,
    pub(super) common_git: PathBuf,
}

impl RepositoryIdentity {
    pub(super) fn capture(repository: &Repository) -> Self {
        Self {
            worktree: None,
            common_git: repository.common_dir().to_path_buf(),
        }
    }

    pub(super) fn validate(&self, repository: &Repository) -> Result<(), InstallError> {
        if self.worktree.is_none() && self.common_git == repository.common_dir() {
            Ok(())
        } else {
            Err(InstallError::InvalidRecord(
                "install record belongs to another repository".to_owned(),
            ))
        }
    }
}

/// Complete bytes, mode, and content identity of one regular file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileImage {
    pub(super) mode: u32,
    pub(super) bytes: Vec<u8>,
    pub(super) digest: Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ObservedFile {
    pub(super) image: FileImage,
    pub(super) generation: Generation,
}

impl ObservedFile {
    pub(super) fn generated(&self) -> GeneratedImage {
        self.image.generated()
    }
}

/// Native and content identity of one observed install receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ReceiptGeneration {
    generation: Generation,
    image: GeneratedImage,
}

impl ReceiptGeneration {
    pub(super) fn observed(observed: &ObservedFile) -> Self {
        Self::published(observed.generation, &observed.image)
    }

    pub(super) fn published(generation: Generation, image: &FileImage) -> Self {
        Self {
            generation,
            image: image.generated(),
        }
    }

    pub(super) const fn native(&self) -> Generation {
        self.generation
    }

    pub(super) fn matches(&self, observed: &ObservedFile) -> bool {
        self.generation == observed.generation && self.image == observed.generated()
    }
}

pub(super) fn images_match(
    observed: Option<&ObservedFile>,
    expected: Option<&GeneratedImage>,
) -> bool {
    // Durable drift belongs to the complete file image. A generation is only
    // an immediate compare-and-swap guard for the mutation that follows.
    match (observed, expected) {
        (None, None) => true,
        (Some(observed), Some(expected)) => observed.generated() == *expected,
        _ => false,
    }
}

impl FileImage {
    pub(super) fn new(mode: u32, bytes: Vec<u8>) -> Self {
        let digest = Digest::compute(Domain::InstallArtifact, &bytes);
        Self {
            mode,
            bytes,
            digest,
        }
    }

    /// Installed permission bits.
    #[must_use]
    pub const fn mode(&self) -> u32 {
        self.mode
    }

    /// Exact file bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Content identity of the file bytes.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    pub(super) fn generated(&self) -> GeneratedImage {
        GeneratedImage {
            mode: self.mode,
            digest: self.digest.clone(),
        }
    }
}

/// Mode and content identity of bytes an install operation will publish.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedImage {
    pub(super) mode: u32,
    pub(super) digest: Digest,
}

impl GeneratedImage {
    pub(super) fn new(mode: u32, bytes: &[u8]) -> Self {
        Self {
            mode,
            digest: Digest::compute(Domain::InstallArtifact, bytes),
        }
    }

    /// Installed permission bits.
    #[must_use]
    pub const fn mode(&self) -> u32 {
        self.mode
    }

    /// Identity of the exact bytes selected by the plan.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    pub(super) fn validate(&self) -> bool {
        self.mode <= 0o777
    }
}

/// One exact file replacement and the state it observed before installation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub(super) asset: Asset,
    pub(super) location: Location,
    pub(super) path: String,
    pub(super) destination: String,
    pub(super) parent: super::ParentObservation,
    /// Fixed-size identity of the pre-install image restored by reversal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) previous: Option<GeneratedImage>,
    pub(super) generated: GeneratedImage,
    /// Installed image this operation replaces, when it refreshes a completed
    /// install rather than introducing one.
    ///
    /// `previous` never moves: it stays the pre-install state a reversal
    /// restores, so refreshing repeatedly still uninstalls to the tree the
    /// first apply found. What a refresh needs beyond that is the image it
    /// expects to *find* in place, which is the superseded build's output
    /// rather than `previous`. Absent for a first install, and skipped from the
    /// canonical bytes then, so plans that introduce an install keep the digest
    /// they had before refreshing existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) superseded: Option<GeneratedImage>,
    pub(super) introduced_parents: Vec<String>,
}

/// One legacy side-channel path removed after every live Spice asset exists.
///
/// Retirements are deliberately separate from [`Operation`]: they publish no
/// current asset and therefore never re-enter the install inventory. Reversible
/// retirements keep the exact prior image so reversal restores the preceding
/// installation; an explicitly discarded obsolete file is never restored.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Retirement {
    pub(super) path: String,
    pub(super) destination: String,
    pub(super) parent: super::ParentObservation,
    /// Fixed-size identity of the image restored by reversal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) previous: Option<GeneratedImage>,
    /// Fixed-size identity the initial retirement may replace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) observed: Option<GeneratedImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) superseded: Option<GeneratedImage>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) discard: bool,
}

const fn is_false(value: &bool) -> bool {
    !*value
}

impl Retirement {
    pub(super) fn validate_binding(&self, repository: &Repository) -> Result<(), InstallError> {
        super::binding::validate_operation_binding(
            repository,
            Location::CommonGit,
            &self.path,
            &self.destination,
            &self.parent,
        )
    }

    /// Retired repository-relative path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Exact prior image identity restored by reversal, if one existed.
    #[must_use]
    pub const fn previous(&self) -> Option<&GeneratedImage> {
        self.previous.as_ref()
    }

    pub(super) fn destination(&self, repository: &Repository) -> PathBuf {
        repository.common_dir().join(&self.destination)
    }

    pub(super) fn expects_in_place(&self, observed: Option<&ObservedFile>) -> bool {
        self.superseded.as_ref().map_or_else(
            || images_match(observed, self.observed.as_ref()),
            |superseded| observed.is_some_and(|observed| &observed.generated() == superseded),
        )
    }

    pub(super) const fn discards_payload(&self) -> bool {
        self.discard
    }

    /// Whether this discard is the last of its planned run over one binding
    /// that names missing parent directories, which it withdraws once empty.
    pub(super) fn withdraws_parent(&self, next: Option<&Self>) -> bool {
        self.discard
            && !self.parent.missing.is_empty()
            && next.is_none_or(|next| next.parent != self.parent)
    }
}

impl Operation {
    pub(super) fn validate_binding(&self, repository: &Repository) -> Result<(), InstallError> {
        super::binding::validate_operation_binding(
            repository,
            self.location,
            &self.path,
            &self.destination,
            &self.parent,
        )
    }

    /// Kind of asset this operation owns.
    #[must_use]
    pub const fn asset(&self) -> Asset {
        self.asset
    }

    /// Root against which [`Self::path`] resolves.
    #[must_use]
    pub const fn location(&self) -> Location {
        self.location
    }

    /// Repository-relative destination spelling.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }
    /// Prior regular-file image identity, or absence when this install introduces it.
    #[must_use]
    pub const fn previous(&self) -> Option<&GeneratedImage> {
        self.previous.as_ref()
    }

    /// Exact image this install places.
    #[must_use]
    pub const fn generated(&self) -> &GeneratedImage {
        &self.generated
    }

    /// Installed image a refresh replaces, absent when this install introduces it.
    #[must_use]
    pub const fn superseded(&self) -> Option<&GeneratedImage> {
        self.superseded.as_ref()
    }

    /// Whether `observed` is the exact state this operation may publish over.
    ///
    /// A first install may publish only over the image it planned against. A
    /// refresh may publish only over the build it supersedes, because the
    /// destination already holds an installed image rather than `previous`.
    pub(super) fn expects_in_place(&self, observed: Option<&ObservedFile>) -> bool {
        match &self.superseded {
            Some(superseded) => {
                observed.is_some_and(|observed| &observed.generated() == superseded)
            }
            None => images_match(observed, self.previous.as_ref()),
        }
    }

    /// Parent directories absent when the plan was derived, outermost first.
    #[must_use]
    pub fn introduced_parents(&self) -> &[String] {
        &self.introduced_parents
    }

    pub(super) fn destination(&self, repository: &Repository) -> PathBuf {
        self.location.root(repository).join(&self.destination)
    }
}
