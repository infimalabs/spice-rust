use super::{
    ConfigurationSource, MeasurementEngine, ToolRequirement, ToolchainDeclaration, TreeInput,
    TreeJurisdiction,
};
use crate::manifest::GateId;

const CURRENT_BUILD_EXCLUDES: &[&str] = &[
    ".git/**",
    "target/**",
    "dist/**",
    "node_modules/**",
    ".venv/**",
    "vendor/**",
];
/// Release-packaged path-part exclusions retained by scanner declarations.
pub const PACKAGED_SCANNER_EXCLUDES: &[&str] = &[
    "any path component equal to .git",
    "any path component starting with .spice",
    "any path component equal to .venv",
    "any path component equal to .ruff_cache",
    "any path component equal to .pytest_cache",
    "any path component equal to __pycache__",
    "any path component equal to node_modules",
    "any path component equal to site-packages",
];
/// Release-packaged environment-language globs retained by scanner declarations.
pub const PACKAGED_ENV_INCLUDES: &[&str] = &[
    "**/*.c",
    "**/*.cc",
    "**/*.cpp",
    "**/*.cs",
    "**/*.go",
    "**/*.h",
    "**/*.hpp",
    "**/*.java",
    "**/*.js",
    "**/*.kt",
    "**/*.m",
    "**/*.mm",
    "**/*.rs",
    "**/*.scala",
    "**/*.swift",
    "**/*.ts",
    "**/*.lua",
    "**/*.php",
    "**/*.py",
    "**/*.rb",
    "**/*.bash",
    "**/*.sh",
    "**/*.zsh",
];
const PACKAGED_ENV_NAME_PATTERNS: &[&str] = &[
    "SPICE_[A-Z0-9_]+",
    "CODEX_THREAD_ID",        // env-policy: allow
    "CLAUDE_CODE_SESSION_ID", // env-policy: allow
];
const PACKAGED_ENV_FAMILY_SUFFIXES: &[(&str, &[&str])] = &[
    ("python", &[".py"]),
    ("csharp", &[".cs"]),
    ("lua", &[".lua"]),
    ("shell", &[".bash", ".sh", ".zsh"]),
    ("javascript", &[".js", ".ts"]),
];
const PACKAGED_ENV_ACCESS_PATTERNS: &[(&str, &[&str])] = &[
    ("python", &[r"\bos\.(?:environ|getenv|putenv|unsetenv)\b"]),
    (
        "csharp",
        &[concat!(
            r"\b(?:System\.)?Environment\.",
            r"(?:GetEnvironmentVariable|SetEnvironmentVariable)\b"
        )],
    ),
    ("lua", &[r"\bos\.getenv\b"]),
    (
        "shell",
        &[
            concat!(
                r"(?<!\\)\$(?:[A-Za-z][A-Za-z0-9_]*|_[A-Za-z0-9_]+|\{",
                r"(?:[A-Za-z][A-Za-z0-9_]*|_[A-Za-z0-9_]+)\})"
            ),
            r"\bexport\s+(?:[A-Za-z][A-Za-z0-9_]*|_[A-Za-z0-9_]+)=",
        ],
    ),
    ("javascript", &[r"\bprocess\.env\b"]),
];
const PACKAGED_ENV_FINDING_NAMES: &[(&str, &str)] = &[
    ("python", "os env access"),
    ("csharp", "environment env access"),
    ("lua", "lua env access"),
    ("shell", "shell env access"),
    ("javascript", "process.env access"),
];

/// Complete packaged environment scanner profile admitted for direct parity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackagedEnvironmentProfile {
    /// Candidate file globs.
    pub includes: &'static [&'static str],
    /// Fixed path-part exclusions.
    pub excludes: &'static [&'static str],
    /// Inline or preceding-statement waiver marker.
    pub allow_marker: &'static str,
    /// Quoted environment-name watchlist patterns.
    pub name_patterns: &'static [&'static str],
    /// Scanner implementation path excluded from self-reporting.
    pub self_path_suffix: &'static str,
    /// Whether every recognized access site requires a waiver.
    pub access_gate: bool,
    /// Language family to suffix mappings.
    pub family_suffixes: &'static [(&'static str, &'static [&'static str])],
    /// Language family to access-pattern mappings.
    pub access_patterns: &'static [(&'static str, &'static [&'static str])],
    /// Stable finding names for access families.
    pub finding_names: &'static [(&'static str, &'static str)],
    /// Packaged exact declarations; release 0.31.0 ships none.
    pub declared_names: &'static [&'static str],
    /// Packaged baseline; release 0.31.0 ships none.
    pub baseline: Option<&'static str>,
}

/// Exact release 0.31.0 environment scanner profile compiled into this boundary.
pub const PACKAGED_ENV_PROFILE: PackagedEnvironmentProfile = PackagedEnvironmentProfile {
    includes: PACKAGED_ENV_INCLUDES,
    excludes: PACKAGED_SCANNER_EXCLUDES,
    allow_marker: "env-policy: allow",
    name_patterns: PACKAGED_ENV_NAME_PATTERNS,
    self_path_suffix: "studies/envpolicy.py",
    access_gate: true,
    family_suffixes: PACKAGED_ENV_FAMILY_SUFFIXES,
    access_patterns: PACKAGED_ENV_ACCESS_PATTERNS,
    finding_names: PACKAGED_ENV_FINDING_NAMES,
    declared_names: &[],
    baseline: None,
};
/// Release-packaged workstation path marker retained by the current scanner profile.
pub const LOCAL_PATH_MARKER: &str = concat!("/", "Users", "/");
/// Release-packaged prose vocabulary retained by the current scanner profile.
pub const PACKAGED_TASTE_WORDS: &[(&str, &str)] = &[
    ("hallucinat*", "confabulate"),
    ("master", "main, primary, or leader"),
    ("orphan*", "loose"),
    ("whitelist", "allowlist"),
    ("whitelists", "allowlists"),
    ("whitelisted", "allowlisted"),
    ("whitelisting", "allowlisting"),
    ("blacklist", "blocklist"),
    ("blacklists", "blocklists"),
    ("blacklisted", "blocklisted"),
    ("blacklisting", "blocklisting"),
];
/// Release-packaged JavaScript declaration exemptions retained by current policy.
pub const PACKAGED_JAVASCRIPT_EXEMPTIONS: &[(&str, &str, &str)] = &[(
    "spice/serve/static/app.mosaic-event-log.js",
    "mosaicReplayEventLog",
    concat!(
        "shared browser replay oracle that exercises the production event-log ",
        "branch implementation without duplicating that algorithm into tests"
    ),
)];
/// Release-packaged JavaScript test root used for test-only classification.
pub const PACKAGED_TEST_PATHS: &[&str] = &["tests"];
const FORMATTER_TOOLS: &[ToolRequirement] = &[
    ToolRequirement {
        name: "cargo",
        program: "cargo",
        version_argv: &["--version"],
        exact_version: "cargo 1.97.1 (c980f4866 2026-06-30)",
        role: "workspace formatter driver",
    },
    ToolRequirement {
        name: "rustfmt",
        program: "rustfmt",
        version_argv: &["--version"],
        exact_version: "rustfmt 1.9.0-stable (8bab26f4f6 2026-07-14)",
        role: "Rust formatter",
    },
];
const CANVAS_TOOLS: &[ToolRequirement] = &[
    ToolRequirement {
        name: "cargo",
        program: "cargo",
        version_argv: &["--version"],
        exact_version: "cargo 1.97.1 (c980f4866 2026-06-30)",
        role: "wasm32 typecheck driver",
    },
    ToolRequirement {
        name: "trunk",
        program: "trunk",
        version_argv: &["--version"],
        exact_version: "trunk 0.21.14",
        role: "canvas asset graph validator",
    },
];
const PYRIGHT_TOOLS: &[ToolRequirement] = &[ToolRequirement {
    name: "pyright",
    program: "pyright",
    version_argv: &["--version"],
    exact_version: "pyright 1.1.411",
    role: "Python type analyzer",
}];

/// All eight declarations in their fixed interval of the predecessor battery.
pub const TOOLCHAIN_ARMS: [ToolchainDeclaration; 8] = [
    ToolchainDeclaration {
        id: GateId::Formatters,
        measurement_engine: MeasurementEngine::ToolchainV1,
        jurisdiction: TreeJurisdiction {
            input: TreeInput::CompleteSelectedTree,
            roots: &["."],
            includes: &["**/*.rs", "Cargo.toml"],
            excludes: CURRENT_BUILD_EXCLUDES,
            applicability: "at least one selected **/*.rs file",
        },
        configuration: ConfigurationSource {
            source: "rust-toolchain.toml",
            field: "toolchain.channel + toolchain.components[rustfmt]",
        },
        tools: FORMATTER_TOOLS,
        analysis: &["cargo fmt --all -- --check"],
    },
    ToolchainDeclaration {
        id: GateId::LocalPaths,
        measurement_engine: MeasurementEngine::ToolchainV1,
        jurisdiction: TreeJurisdiction {
            input: TreeInput::SelectedChanges,
            roots: &["."],
            includes: &["regular files decoded as UTF-8 with replacement"],
            excludes: PACKAGED_SCANNER_EXCLUDES,
            applicability: "at least one selected UTF-8 text file",
        },
        configuration: ConfigurationSource {
            source: "gate crate",
            field: "gate::toolchain::LOCAL_PATH_MARKER + PACKAGED_SCANNER_EXCLUDES",
        },
        tools: &[],
        analysis: &["compiled LOCAL_PATH_MARKER scan over selected changes"],
    },
    ToolchainDeclaration {
        id: GateId::Taste,
        measurement_engine: MeasurementEngine::ToolchainV1,
        jurisdiction: TreeJurisdiction {
            input: TreeInput::SelectedChanges,
            roots: &["."],
            includes: &["**/*.md", "**/*.txt", "**/*.rst"],
            excludes: PACKAGED_SCANNER_EXCLUDES,
            applicability: "at least one selected prose file",
        },
        configuration: ConfigurationSource {
            source: "gate crate",
            field: "gate::toolchain::PACKAGED_TASTE_WORDS + PACKAGED_SCANNER_EXCLUDES",
        },
        tools: &[],
        analysis: &["compiled PACKAGED_TASTE_WORDS scan over selected changes"],
    },
    ToolchainDeclaration {
        id: GateId::CanvasWebTypecheck,
        measurement_engine: MeasurementEngine::ToolchainV1,
        jurisdiction: TreeJurisdiction {
            input: TreeInput::CompleteSelectedTree,
            roots: &["."],
            includes: &[
                "Cargo.toml",
                "Cargo.lock",
                "**/Cargo.toml",
                "**/src/**/*.rs",
                "app/Cargo.toml",
                "app/index.html",
                "app/**/*.css",
            ],
            excludes: CURRENT_BUILD_EXCLUDES,
            applicability: "workspace Cargo.toml and app/index.html are selected",
        },
        configuration: ConfigurationSource {
            source: "rust-toolchain.toml",
            field: "toolchain.channel + toolchain.targets[wasm32-unknown-unknown]",
        },
        tools: CANVAS_TOOLS,
        analysis: &[
            "cargo check --locked --workspace --target wasm32-unknown-unknown",
            "trunk build index.html --release --locked --filehash=false",
        ],
    },
    ToolchainDeclaration {
        id: GateId::JavascriptUnused,
        measurement_engine: MeasurementEngine::ToolchainV1,
        jurisdiction: TreeJurisdiction {
            input: TreeInput::CompleteSelectedTree,
            roots: &["."],
            includes: &["**/*.js", "**/*.mjs", "**/*.cjs"],
            excludes: PACKAGED_SCANNER_EXCLUDES,
            applicability: "at least one selected JavaScript module",
        },
        configuration: ConfigurationSource {
            source: "gate crate",
            field: concat!(
                "gate::toolchain::PACKAGED_JAVASCRIPT_EXEMPTIONS + ",
                "PACKAGED_TEST_PATHS + PACKAGED_SCANNER_EXCLUDES"
            ),
        },
        tools: &[],
        analysis: &["supplied JavaScript declaration/reference normalization"],
    },
    ToolchainDeclaration {
        id: GateId::PythonTypecheck,
        measurement_engine: MeasurementEngine::ToolchainV1,
        jurisdiction: TreeJurisdiction {
            input: TreeInput::CompleteSelectedTree,
            roots: &["."],
            includes: &["pyproject.toml", "**/*.py", "**/*.pyi"],
            excludes: CURRENT_BUILD_EXCLUDES,
            applicability: "pyproject.toml and at least one selected Python source file",
        },
        configuration: ConfigurationSource {
            source: "pyproject.toml",
            field: "tool.pyright",
        },
        tools: PYRIGHT_TOOLS,
        analysis: &["pyright --level error"],
    },
    ToolchainDeclaration {
        id: GateId::EnvPolicy,
        measurement_engine: MeasurementEngine::ToolchainV1,
        jurisdiction: TreeJurisdiction {
            input: TreeInput::SelectedChanges,
            roots: &["."],
            includes: PACKAGED_ENV_INCLUDES,
            excludes: PACKAGED_SCANNER_EXCLUDES,
            applicability: "at least one selected packaged env-language source file",
        },
        configuration: ConfigurationSource {
            source: "gate crate",
            field: "gate::toolchain::PACKAGED_ENV_PROFILE",
        },
        tools: &[],
        analysis: &["supplied environment access finding normalization"],
    },
    ToolchainDeclaration {
        id: GateId::EnvNameLedger,
        measurement_engine: MeasurementEngine::ToolchainV1,
        jurisdiction: TreeJurisdiction {
            input: TreeInput::CompleteSelectedTree,
            roots: &["."],
            includes: PACKAGED_ENV_INCLUDES,
            excludes: PACKAGED_SCANNER_EXCLUDES,
            applicability: "at least one tracked packaged env-language source file",
        },
        configuration: ConfigurationSource {
            source: "gate crate",
            field: "gate::toolchain::PACKAGED_ENV_PROFILE",
        },
        tools: &[],
        analysis: &["supplied environment ledger finding normalization"],
    },
];
