//! Repository-authored configuration carried into compiled Gate policy.

use serde::{Deserialize, Serialize};

/// The narrowest file-class declaration a policy document can be loaded with.
///
/// Parsing requires all three selections, so no document loads without them
/// and anything that needs a valid document for some other purpose has to
/// carry them. That text lives here once instead of being restated at each
/// such place, because a restatement drifts from the parser silently: it keeps
/// working until the day the required spelling changes, and then fails
/// somewhere that has nothing to do with the change.
pub const NARROWEST_FILE_CLASSES: &str = concat!(
    "[policy.file_shape]\n",
    "source_suffixes = [\".rs\"]\n",
    "[policy.languages]\n",
    "complexity = [\".rs\"]\n",
    "magic = [\".rs\"]\n",
);

/// Packaged file-class policy used only when a generic installed repository
/// has no Rust Spice policy document at all.
///
/// This is not a fallback for an incomplete document: once a repository
/// authors its tracked policy document, all three selections remain required and the
/// exact tracked field refuses if one is absent. The generic profile selects
/// ordinary text/source shapes broadly, while the parser-backed classes name
/// only suffixes this build can measure exactly. That keeps packaged built-ins
/// useful across languages without converting an unsupported parser into a
/// repository failure.
pub const PACKAGED_GENERIC_POLICY: &str = concat!(
    "[policy.file_shape]\n",
    "source_suffixes = [",
    "\".astro\", \".bash\", \".c\", \".cc\", \".cjs\", \".cpp\", ",
    "\".cs\", \".css\", \".cts\", \".go\", \".h\", \".hpp\", ",
    "\".html\", \".java\", \".js\", \".json\", \".jsx\", \".kt\", ",
    "\".less\", \".lua\", \".m\", \".md\", \".mjs\", \".mm\", ",
    "\".mts\", \".php\", \".py\", \".pyi\", \".rb\", \".rs\", ",
    "\".rst\", \".sass\", \".scala\", \".scss\", \".sh\", \".sql\", ",
    "\".svelte\", \".swift\", \".toml\", \".ts\", \".tsx\", \".txt\", ",
    "\".vue\", \".xml\", \".yaml\", \".yml\", \".zsh\"",
    "]\n",
    "[policy.languages]\n",
    "complexity = [\".cs\", \".js\", \".lua\", \".mjs\", \".py\", \".rs\"]\n",
    "magic = [",
    "\".c\", \".cc\", \".cpp\", \".go\", \".h\", \".hpp\", ",
    "\".java\", \".kt\", \".m\", \".mm\", \".rs\", \".scala\", ",
    "\".swift\", \".ts\"",
    "]\n",
);

/// One repository's declaration of the documents that carry its doctrine.
///
/// An absent `docs` is not an empty inventory: it is the packaged one. That is
/// the whole reason this is an option rather than a defaulted list, because a
/// repository that wants nothing measured has to be able to say so distinctly
/// from a repository that has said nothing at all.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RepoTruthPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    docs: Option<Vec<String>>,
}

impl RepoTruthPolicy {
    /// Documents this repository names, or `None` when it names none.
    #[must_use]
    pub fn docs(&self) -> Option<&[String]> {
        self.docs.as_deref()
    }
}

/// One repository's declaration of which files carry measurable shape.
///
/// Unlike [`RepoTruthPolicy`], an absent `source_suffixes` is not a packaged
/// inventory this can fall back to. The shape arm measures whatever this
/// selects, so a repository that has said nothing would silently select
/// nothing and pass; the consumer that compiles it refuses instead.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileShapePolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    source_suffixes: Option<Vec<String>>,
}

impl FileShapePolicy {
    /// Suffixes this repository measures shape over, or `None` when unsaid.
    #[must_use]
    pub fn source_suffixes(&self) -> Option<&[String]> {
        self.source_suffixes.as_deref()
    }
}

/// One repository's declaration of which files each language arm reads.
///
/// The two selections are separate because they are genuinely different sets:
/// a language whose routines this repository wants counted is not necessarily
/// one whose literals it wants a magic-number delta over.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LanguagesPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    complexity: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    magic: Option<Vec<String>>,
}

impl LanguagesPolicy {
    /// Suffixes whose routines are measured, or `None` when unsaid.
    #[must_use]
    pub fn complexity(&self) -> Option<&[String]> {
        self.complexity.as_deref()
    }

    /// Suffixes whose literals are measured, or `None` when unsaid.
    #[must_use]
    pub fn magic(&self) -> Option<&[String]> {
        self.magic.as_deref()
    }
}

/// One repository's declaration of the shape its module names take.
///
/// A present but blank pattern is a different fact than a key nobody wrote, so
/// this stays an option and the consumer that compiles it refuses the first
/// while admitting the second.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PackagePolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    boundary_underscore_pattern: Option<String>,
}

impl PackagePolicy {
    /// Module-name shape this repository names, or `None` when it names none.
    #[must_use]
    pub fn boundary_underscore_pattern(&self) -> Option<&str> {
        self.boundary_underscore_pattern.as_deref()
    }
}
