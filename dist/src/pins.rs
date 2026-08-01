//! Exact build-tool pin admission.

use std::fmt;

use wire::manifest::BuildPins;

const SHA256_HEX_LEN: usize = 64;

/// One pin in the fixed pre-build comparison order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinName {
    Rust,
    WasmTarget,
    Trunk,
    CargoMutants,
    ProofContainer,
}

impl PinName {
    const ALL: [Self; 5] = [
        Self::Rust,
        Self::WasmTarget,
        Self::Trunk,
        Self::CargoMutants,
        Self::ProofContainer,
    ];

    fn value(self, pins: &BuildPins) -> &str {
        match self {
            Self::Rust => &pins.rust,
            Self::WasmTarget => &pins.wasm_target,
            Self::Trunk => &pins.trunk,
            Self::CargoMutants => &pins.cargo_mutants,
            Self::ProofContainer => &pins.proof_container,
        }
    }
}

impl fmt::Display for PinName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Rust => "rust",
            Self::WasmTarget => "wasm-target",
            Self::Trunk => "trunk",
            Self::CargoMutants => "cargo-mutants",
            Self::ProofContainer => "proof-container",
        })
    }
}

/// Validated exact pins required by one pipeline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequiredPins(BuildPins);

impl RequiredPins {
    /// Admit a complete pin set.
    ///
    /// # Errors
    ///
    /// Returns [`PinError::Empty`] when any identity is blank and
    /// [`PinError::ContainerNotDigestPinned`] when the proof image is not an
    /// OCI digest reference.
    pub fn new(pins: BuildPins) -> Result<Self, PinError> {
        for name in PinName::ALL {
            if name.value(&pins).trim().is_empty() {
                return Err(PinError::Empty { name });
            }
        }
        if !digest_pinned_container(&pins.proof_container) {
            return Err(PinError::ContainerNotDigestPinned {
                observed: pins.proof_container,
            });
        }
        Ok(Self(pins))
    }

    /// Compare observed identities before any build work starts.
    ///
    /// # Errors
    ///
    /// Returns the first mismatch in the fixed comparison order.
    pub fn verify(&self, observed: &BuildPins) -> Result<(), PinError> {
        for name in PinName::ALL {
            let expected = name.value(&self.0);
            let actual = name.value(observed);
            if expected != actual {
                return Err(PinError::Mismatch {
                    name,
                    expected: expected.to_owned(),
                    observed: actual.to_owned(),
                });
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn get(&self) -> &BuildPins {
        &self.0
    }
}

/// A refusal raised before build execution.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PinError {
    #[error("required {name} pin is empty")]
    Empty { name: PinName },
    #[error("proof container must use name@sha256:<lower-case SHA-256 hex>: {observed:?}")]
    ContainerNotDigestPinned { observed: String },
    #[error("{name} pin mismatch: expected {expected:?}, observed {observed:?}")]
    Mismatch {
        name: PinName,
        expected: String,
        observed: String,
    },
}

fn digest_pinned_container(value: &str) -> bool {
    let Some((name, digest)) = value.rsplit_once("@sha256:") else {
        return false;
    };
    !name.is_empty()
        && digest.len() == SHA256_HEX_LEN
        && digest.bytes().all(|byte| {
            byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit())
        })
}
