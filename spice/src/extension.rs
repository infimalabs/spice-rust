//! One deterministic registry for compiled vendors, wrappers, and gates.
//!
//! The registry is a discovery boundary, not a dispatcher. Each declaration
//! carries a typed value whose kind must agree with its namespace. Discovery
//! sorts before validating so a collision or malformed mixed declaration is
//! reported identically regardless of link order.

use std::error::Error;
use std::fmt;
use std::ops::Deref;

/// Closed extension namespaces understood by this release.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Namespace {
    /// Agent CLI vendors.
    Vendor,
    /// Shell command wrappers.
    Wrapper,
    /// Repository gates.
    Gate,
}

impl Namespace {
    /// Stable entry-point group spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vendor => "spice.vendors",
            Self::Wrapper => "spice.wrappers",
            Self::Gate => "spice.gates",
        }
    }
}

/// One wrapper value declared by its owning compiled component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wrapper {
    /// Component that executes the wrapper contract.
    pub owner: &'static str,
}

/// One gate value declared by its owning compiled component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gate {
    /// Component that executes the gate contract.
    pub owner: &'static str,
}

/// Typed configuration fold owned by one declared vendor.
pub type ConfigFold = for<'a> fn(&'a crate::config::Root) -> vendor::LaunchValues<'a>;

/// Session-gate behavior carried by one declared vendor value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionGate {
    identity: &'static str,
    dialect: listen::Dialect,
    ambient_precedence: u16,
}

impl SessionGate {
    const fn new(
        identity: &'static str,
        dialect: listen::Dialect,
        ambient_precedence: u16,
    ) -> Self {
        Self {
            identity,
            dialect,
            ambient_precedence,
        }
    }

    /// Stable identity retained in session reports.
    #[must_use]
    pub const fn identity(self) -> &'static str {
        self.identity
    }

    /// Transcript decoder selected for declared sources.
    #[must_use]
    pub const fn dialect(self) -> listen::Dialect {
        self.dialect
    }

    /// Deterministic priority among simultaneous ambient identities.
    #[must_use]
    pub const fn ambient_precedence(self) -> u16 {
        self.ambient_precedence
    }
}

/// One complete declared vendor, including configuration and session gate.
#[derive(Clone, Copy)]
pub struct Vendor {
    dialect: &'static vendor::Vendor,
    configuration: ConfigFold,
    session_gate: SessionGate,
}

impl Vendor {
    /// Bind one raw vendor contract to configuration and ambient gate order.
    #[must_use]
    pub const fn new(
        dialect: &'static vendor::Vendor,
        configuration: ConfigFold,
        ambient_precedence: u16,
    ) -> Self {
        Self {
            dialect,
            configuration,
            session_gate: SessionGate::new(dialect.name(), dialect.dialect(), ambient_precedence),
        }
    }

    /// Project launch values from the authoritative configuration root.
    #[must_use]
    pub fn launch_values<'a>(&self, root: &'a crate::config::Root) -> vendor::LaunchValues<'a> {
        (self.configuration)(root)
    }

    /// Declaration-owned session gate contract.
    #[must_use]
    pub const fn session_gate(&self) -> SessionGate {
        self.session_gate
    }
}

impl Deref for Vendor {
    type Target = vendor::Vendor;

    fn deref(&self) -> &Self::Target {
        self.dialect
    }
}

impl fmt::Debug for Vendor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.dialect.fmt(formatter)
    }
}

impl PartialEq for Vendor {
    fn eq(&self, other: &Self) -> bool {
        self.dialect == other.dialect
    }
}

impl Eq for Vendor {}

/// Current Codex declaration.
pub const CODEX: Vendor = Vendor::new(&vendor::CODEX, codex_configuration, 0);

/// Current Claude declaration.
pub const CLAUDE: Vendor = Vendor::new(&vendor::CLAUDE, claude_configuration, 1);

/// Current oh-my-pi declaration.
pub const OMP: Vendor = Vendor::new(&vendor::OMP, omp_configuration, 2);

/// Typed payload of one extension declaration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Value {
    /// Complete agent vendor value.
    Vendor(&'static Vendor),
    /// Shell wrapper value.
    Wrapper(Wrapper),
    /// Repository gate value.
    Gate(Gate),
}

impl Value {
    const fn namespace(self) -> Namespace {
        match self {
            Self::Vendor(_) => Namespace::Vendor,
            Self::Wrapper(_) => Namespace::Wrapper,
            Self::Gate(_) => Namespace::Gate,
        }
    }
}

/// One installed extension declaration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Declaration {
    /// Namespace in which `name` must be unique.
    pub namespace: Namespace,
    /// Stable selected name.
    pub name: &'static str,
    /// Distribution or compiled component supplying the declaration.
    pub provider: &'static str,
    /// Typed declared value.
    pub value: Value,
}

/// Current compiled declarations. These are values, not mode arms: consumers
/// resolve a row and call the selected value's contract.
pub const DECLARATIONS: &[Declaration] = &[
    Declaration {
        namespace: Namespace::Vendor,
        name: "codex",
        provider: "vendor",
        value: Value::Vendor(&CODEX),
    },
    Declaration {
        namespace: Namespace::Vendor,
        name: "claude",
        provider: "vendor",
        value: Value::Vendor(&CLAUDE),
    },
    Declaration {
        namespace: Namespace::Vendor,
        name: "omp",
        provider: "vendor",
        value: Value::Vendor(&OMP),
    },
    Declaration {
        namespace: Namespace::Wrapper,
        name: "shell",
        provider: "steer::run",
        value: Value::Wrapper(Wrapper {
            owner: "steer::run",
        }),
    },
    Declaration {
        namespace: Namespace::Gate,
        name: "core",
        provider: "gate::core",
        value: Value::Gate(Gate {
            owner: "gate::core",
        }),
    },
    Declaration {
        namespace: Namespace::Gate,
        name: "hygiene",
        provider: "gate::hygiene",
        value: Value::Gate(Gate {
            owner: "gate::hygiene",
        }),
    },
    Declaration {
        namespace: Namespace::Gate,
        name: "toolchain",
        provider: "gate::toolchain",
        value: Value::Gate(Gate {
            owner: "gate::toolchain",
        }),
    },
];

/// Deterministically discovered extension rows.
#[derive(Clone, Debug)]
pub struct Registry<'a> {
    rows: Vec<&'a Declaration>,
}

impl<'a> Registry<'a> {
    /// Discover and validate declarations without loading or executing them.
    ///
    /// # Errors
    ///
    /// Returns the first deterministic empty, mixed, mismatched, or colliding row.
    pub fn discover(declarations: &'a [Declaration]) -> Result<Self, RegistryError> {
        let mut rows = declarations.iter().collect::<Vec<_>>();
        rows.sort_by_key(|row| (row.namespace, row.name, row.provider));
        let mut previous = None;
        for row in &rows {
            if row.name.trim().is_empty() {
                return Err(RegistryError::EmptyName {
                    namespace: row.namespace,
                    provider: row.provider.to_owned(),
                });
            }
            if row.provider.trim().is_empty() {
                return Err(RegistryError::EmptyProvider {
                    namespace: row.namespace,
                    name: row.name.to_owned(),
                });
            }
            let value_namespace = row.value.namespace();
            if value_namespace != row.namespace {
                return Err(RegistryError::Mixed {
                    namespace: row.namespace,
                    name: row.name.to_owned(),
                    value_namespace,
                    provider: row.provider.to_owned(),
                });
            }
            if let Value::Vendor(vendor) = row.value
                && vendor.name() != row.name
            {
                return Err(RegistryError::NameMismatch {
                    namespace: row.namespace,
                    name: row.name.to_owned(),
                    value_name: vendor.name(),
                    provider: row.provider.to_owned(),
                });
            }
            let slot = (row.namespace, row.name);
            if previous == Some(slot) {
                let providers = rows
                    .iter()
                    .filter(|candidate| {
                        candidate.namespace == row.namespace && candidate.name == row.name
                    })
                    .map(|candidate| candidate.provider)
                    .collect();
                return Err(RegistryError::Collision {
                    namespace: row.namespace,
                    name: row.name.to_owned(),
                    providers,
                });
            }
            previous = Some(slot);
        }
        Ok(Self { rows })
    }

    /// All declarations in deterministic namespace/name/provider order.
    #[must_use]
    pub fn rows(&self) -> &[&'a Declaration] {
        &self.rows
    }

    /// Resolve one vendor value without a mode branch.
    #[must_use]
    pub fn vendor(&self, name: &str) -> Option<&'static Vendor> {
        self.rows.iter().find_map(|row| {
            if row.namespace == Namespace::Vendor && row.name == name {
                match row.value {
                    Value::Vendor(vendor) => Some(vendor),
                    Value::Wrapper(_) | Value::Gate(_) => None,
                }
            } else {
                None
            }
        })
    }

    /// Every vendor value in deterministic name order.
    #[must_use]
    pub fn vendors(&self) -> Vec<&'static Vendor> {
        self.rows
            .iter()
            .filter_map(|row| match row.value {
                Value::Vendor(vendor) => Some(vendor),
                Value::Wrapper(_) | Value::Gate(_) => None,
            })
            .collect()
    }

    /// Every vendor in its declaration-owned ambient session order.
    #[must_use]
    pub fn session_vendors(&self) -> Vec<&'static Vendor> {
        let mut vendors = self.vendors();
        vendors.sort_by_key(|vendor| {
            (
                vendor.session_gate().ambient_precedence(),
                vendor.session_gate().identity(),
            )
        });
        vendors
    }
}

/// Discover the current compiled registry.
///
/// # Errors
///
/// Returns a deterministic declaration diagnostic when the binary was linked
/// with an incoherent namespace.
pub fn registry() -> Result<Registry<'static>, RegistryError> {
    Registry::discover(DECLARATIONS)
}

/// Resolve one current compiled vendor.
#[must_use]
pub fn vendor(name: &str) -> Option<&'static Vendor> {
    registry()
        .expect("compiled extension declarations are validated by the test gate")
        .vendor(name)
}

/// An incoherent extension registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryError {
    /// A declaration had no name.
    EmptyName {
        namespace: Namespace,
        provider: String,
    },
    /// A declaration had no provider identity.
    EmptyProvider { namespace: Namespace, name: String },
    /// The namespace and loaded value kind disagreed.
    Mixed {
        namespace: Namespace,
        name: String,
        value_namespace: Namespace,
        provider: String,
    },
    /// A typed value's own name disagreed with its declaration row.
    NameMismatch {
        namespace: Namespace,
        name: String,
        value_name: &'static str,
        provider: String,
    },
    /// More than one provider occupied a namespace/name slot.
    Collision {
        namespace: Namespace,
        name: String,
        providers: Vec<&'static str>,
    },
}

impl RegistryError {
    /// Namespace whose discovery failed.
    #[must_use]
    pub const fn namespace(&self) -> Namespace {
        match self {
            Self::EmptyName { namespace, .. }
            | Self::EmptyProvider { namespace, .. }
            | Self::Mixed { namespace, .. }
            | Self::NameMismatch { namespace, .. }
            | Self::Collision { namespace, .. } => *namespace,
        }
    }
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName {
                namespace,
                provider,
            } => write!(
                formatter,
                "extension group {:?} has an empty entry from {provider:?}",
                namespace.as_str()
            ),
            Self::EmptyProvider { namespace, name } => write!(
                formatter,
                "extension group {:?} entry {name:?} has no provider",
                namespace.as_str()
            ),
            Self::Mixed {
                namespace,
                name,
                value_namespace,
                provider,
            } => write!(
                formatter,
                "extension group {:?} entry {name:?} from {provider:?} loaded a {:?} value",
                namespace.as_str(),
                value_namespace.as_str()
            ),
            Self::NameMismatch {
                namespace,
                name,
                value_name,
                provider,
            } => write!(
                formatter,
                "extension group {:?} entry {name:?} from {provider:?} loaded value {value_name:?}",
                namespace.as_str()
            ),
            Self::Collision {
                namespace,
                name,
                providers,
            } => write!(
                formatter,
                "duplicate extension group {:?} entry {name:?}; providers: {}",
                namespace.as_str(),
                providers.join(", ")
            ),
        }
    }
}

impl Error for RegistryError {}

fn codex_configuration(root: &crate::config::Root) -> vendor::LaunchValues<'_> {
    let configured = root.vendor().codex();
    vendor::LaunchValues {
        binary: configured.binary().as_str(),
        model: configured.model(),
        effort: configured.effort(),
        personality: configured.personality(),
        fast_mode: Some(configured.fast_mode()),
        model_context_window: Some(configured.model_context_window()),
        auto_compact_window_tokens: Some(configured.auto_compact_window_tokens()),
        lsp: None,
    }
}

fn claude_configuration(root: &crate::config::Root) -> vendor::LaunchValues<'_> {
    let configured = root.vendor().claude();
    vendor::LaunchValues {
        binary: configured.binary().as_str(),
        model: configured.model(),
        effort: configured.effort(),
        personality: None,
        fast_mode: None,
        model_context_window: None,
        auto_compact_window_tokens: Some(configured.auto_compact_window_tokens()),
        lsp: None,
    }
}

fn omp_configuration(root: &crate::config::Root) -> vendor::LaunchValues<'_> {
    let configured = root.vendor().omp();
    vendor::LaunchValues {
        binary: configured.binary().as_str(),
        model: configured.model(),
        effort: configured.effort(),
        personality: None,
        fast_mode: Some(configured.fast_mode()),
        model_context_window: None,
        auto_compact_window_tokens: Some(configured.auto_compact_window_tokens()),
        lsp: Some(configured.lsp()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_DIALECT: vendor::Vendor = vendor::Vendor::named("fixture", &vendor::CODEX);
    const FIXTURE_VENDOR: Vendor = Vendor::new(&FIXTURE_DIALECT, codex_configuration, 3);

    #[test]
    fn compiled_namespaces_are_complete_sorted_and_coherent() {
        let registry = registry().expect("compiled declarations are coherent");
        assert_eq!(
            registry
                .rows()
                .iter()
                .map(|row| (row.namespace.as_str(), row.name))
                .collect::<Vec<_>>(),
            [
                ("spice.vendors", "claude"),
                ("spice.vendors", "codex"),
                ("spice.vendors", "omp"),
                ("spice.wrappers", "shell"),
                ("spice.gates", "core"),
                ("spice.gates", "hygiene"),
                ("spice.gates", "toolchain"),
            ],
        );
        assert_eq!(
            registry
                .session_vendors()
                .into_iter()
                .map(|vendor| vendor.session_gate().identity())
                .collect::<Vec<_>>(),
            ["codex", "claude", "omp"],
        );
    }

    #[test]
    fn built_in_shadows_and_extension_duplicates_share_one_refusal() {
        let external = Declaration {
            provider: "external",
            ..DECLARATIONS[0]
        };
        let expected = RegistryError::Collision {
            namespace: Namespace::Vendor,
            name: "codex".to_owned(),
            providers: vec!["external", "vendor"],
        };
        for rows in [[DECLARATIONS[0], external], [external, DECLARATIONS[0]]] {
            assert_eq!(
                Registry::discover(&rows).expect_err("duplicate must refuse"),
                expected,
            );
        }
    }

    #[test]
    fn a_declared_vendor_name_cannot_disagree_with_its_value() {
        let rows = [Declaration {
            namespace: Namespace::Vendor,
            name: "other",
            provider: "fixture",
            value: Value::Vendor(&CODEX),
        }];
        assert!(matches!(
            Registry::discover(&rows),
            Err(RegistryError::NameMismatch {
                name,
                value_name: "codex",
                ..
            }) if name == "other"
        ));
    }

    #[test]
    fn a_third_value_carries_every_vendor_owned_contract() {
        let declarations = [Declaration {
            namespace: Namespace::Vendor,
            name: "fixture",
            provider: "fixture",
            value: Value::Vendor(&FIXTURE_VENDOR),
        }];
        let registry = Registry::discover(&declarations).expect("third declaration is coherent");
        let fixture = registry
            .vendor("fixture")
            .expect("third declaration resolves");
        let bytes = br#"{"type":"thread.started","thread_id":"fixture-thread"}"#;
        let locus = listen::Locus::new(1, 0, bytes.len() as u64);
        let decoded = fixture
            .decode(bytes, locus)
            .expect("fixture record decodes");
        let session = fixture
            .session_declaration(&decoded)
            .expect("fixture identity is valid")
            .expect("fixture record declares a session");

        assert_eq!(fixture.name(), "fixture");
        assert_eq!(fixture.dialect(), listen::Dialect::Codex);
        assert_eq!(fixture.session_gate().identity(), "fixture");
        assert_eq!(fixture.session_gate().dialect(), listen::Dialect::Codex);
        assert_eq!(fixture.session_gate().ambient_precedence(), 3);
        assert_eq!(fixture.stdout(), vendor::CODEX.stdout());
        assert_eq!(fixture.tool_envelope(), vendor::ToolEnvelope::Direct);
        assert_eq!(
            fixture.transcript_layout(),
            vendor::CODEX.transcript_layout()
        );
        assert_eq!(session.session.as_str(), "fixture-thread");
        assert_eq!(session.locus, locus);
        assert_eq!(
            fixture.neutral_launch_prompt(std::path::Path::new("skill.md")),
            "[$spice](<skill.md>)"
        );
        assert_eq!(
            fixture
                .launch_values(&crate::config::Root::default())
                .binary,
            "codex"
        );
    }

    #[test]
    fn mixed_entry_points_are_visible_before_any_value_executes() {
        let rows = [Declaration {
            namespace: Namespace::Gate,
            name: "wrong-kind",
            provider: "fixture",
            value: Value::Vendor(&CODEX),
        }];
        assert!(matches!(
            Registry::discover(&rows),
            Err(RegistryError::Mixed {
                namespace: Namespace::Gate,
                value_namespace: Namespace::Vendor,
                ..
            })
        ));
    }
}
