//! Build-qualified product identity shared by native version surfaces.

use std::fs::File;

use wire::manifest::ProductVersion;
use wire::{Digest, Domain, Fault, FaultClass};

/// Qualify the tracked release version with the exact running artifact identity.
#[must_use]
pub fn product_version(binary: &Digest) -> ProductVersion {
    qualify(
        &dist::current_product_version()
            .expect("the compiled product version is repository-validated"),
        binary,
    )
}

fn qualify(version: &ProductVersion, binary: &Digest) -> ProductVersion {
    let separator = if version.as_str().contains('+') {
        '.'
    } else {
        '+'
    };
    ProductVersion::parse(&format!("{version}{separator}bin.{binary}"))
        .expect("a canonical version and artifact digest form canonical build metadata")
}

/// Hash the executable without consulting a repository or retaining its bytes.
///
/// # Errors
///
/// Reports an environment fault when the running executable cannot be resolved
/// or read. No unqualified version is substituted for missing build evidence.
pub fn binary_digest() -> Result<Digest, Fault> {
    let executable = std::env::current_exe().map_err(|error| Fault {
        class: FaultClass::Environment,
        reason: "version::binary-unresolved".to_owned(),
        detail: error.to_string(),
    })?;
    let read = || {
        let file = File::open(&executable)?;
        let limit = file.metadata()?.len();
        Digest::compute_reader(Domain::DistArtifact, file, limit).map(|(_, digest)| digest)
    };
    read().map_err(|error: std::io::Error| Fault {
        class: FaultClass::Environment,
        reason: "version::binary-unreadable".to_owned(),
        detail: format!("{}: {error}", executable.display()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualification_preserves_existing_metadata_and_distinguishes_artifacts() {
        let first = Digest::compute(Domain::DistArtifact, b"first build");
        let second = Digest::compute(Domain::DistArtifact, b"second build");
        let version = ProductVersion::parse("1.2.3-rc.1+vendor.01").expect("fixture version");
        let qualified = qualify(&version, &first);
        assert!(qualified.as_str().starts_with("1.2.3-rc.1+vendor.01."));
        assert!(qualified.as_str().ends_with(first.as_str()));
        assert_ne!(qualified, qualify(&version, &second));
        let encoded = wire::encode(&qualified).expect("qualified version encodes");
        assert_eq!(wire::decode::<ProductVersion>(&encoded).unwrap(), qualified);
    }
}
