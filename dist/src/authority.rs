//! One tracked Git authority for external Spice releases.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::manifest::ProductVersion;

use crate::version::TrackedVersionError;

const AUTHORITY_BYTES: &[u8] = ::std::include_bytes!("../release-authority.json");
const AUTHORITY_SCHEMA: u16 = 1;
const PACKAGE_NAME: &str = "spice";
const REMOTE: &str = "origin";
const SOURCE_REF: &str = "refs/heads/main";
const PACKAGE_REF: &str = "refs/spice/packages/vVERSION";
const TAG_REF: &str = "refs/tags/vVERSION";
const ANNOUNCEMENT_REF: &str = "refs/spice/releases/vVERSION";
const VERSION_FILE: &str = "dist/VERSION";
const VERSION_SLOT: &str = "VERSION";
const ASCII_DELETE: u8 = 0x7f;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Provider {
    Git,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct AuthorityDocument {
    schema: u16,
    provider: Provider,
    package: String,
    remote: String,
    source: String,
    package_ref: String,
    tag_ref: String,
    announcement_ref: String,
    version_file: String,
}

/// Expanded immutable refs for one release version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseRefs {
    source: String,
    package: String,
    tag: String,
    announcement: String,
}

impl ReleaseRefs {
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn package(&self) -> &str {
        &self.package
    }

    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    #[must_use]
    pub fn announcement(&self) -> &str {
        &self.announcement
    }
}

/// The sole admitted external release authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseAuthority {
    document: AuthorityDocument,
}

impl ReleaseAuthority {
    #[must_use]
    pub fn package_name(&self) -> &str {
        &self.document.package
    }

    #[must_use]
    pub fn remote(&self) -> &str {
        &self.document.remote
    }

    #[must_use]
    pub fn refs(&self, version: &ProductVersion) -> ReleaseRefs {
        ReleaseRefs {
            source: self.document.source.clone(),
            package: expand(&self.document.package_ref, version),
            tag: expand(&self.document.tag_ref, version),
            announcement: expand(&self.document.announcement_ref, version),
        }
    }

    /// Rehearse the exact guarded package publication without changing refs.
    ///
    /// # Errors
    ///
    /// Refuses a noncanonical Git object spelling.
    pub fn package_rehearsal(
        &self,
        version: &ProductVersion,
        object: &str,
    ) -> Result<GitCommand, AuthorityError> {
        object_id(object)?;
        let reference = self.refs(version).package;
        Ok(push_command(
            &self.document.remote,
            object,
            &reference,
            Some(None),
            true,
        ))
    }

    /// Observe the configured remote through the authenticated Git transport.
    #[must_use]
    pub fn remote_visibility(&self) -> GitCommand {
        visibility_command(&self.document.remote, None)
    }

    /// Observe one exact ref through the authenticated Git transport.
    ///
    /// # Errors
    ///
    /// Refuses a noncanonical Git reference spelling.
    pub fn reference_visibility(&self, reference: &str) -> Result<GitCommand, AuthorityError> {
        validate_ref(reference)?;
        Ok(visibility_command(&self.document.remote, Some(reference)))
    }

    /// Observe one exact package ref through the authenticated Git transport.
    #[must_use]
    pub fn package_visibility(&self, version: &ProductVersion) -> GitCommand {
        visibility_command(&self.document.remote, Some(&self.refs(version).package))
    }

    /// Converge the package ref without overwriting another release object.
    ///
    /// # Errors
    ///
    /// Refuses a noncanonical object or an observed different object.
    pub fn package_push(
        &self,
        version: &ProductVersion,
        object: &str,
        state: RefState<'_>,
    ) -> Result<GitMutation, AuthorityError> {
        self.converge(self.refs(version).package, object, state)
    }

    /// Converge the immutable version tag.
    ///
    /// # Errors
    ///
    /// Refuses a noncanonical object or an observed different object.
    pub fn tag_push(
        &self,
        version: &ProductVersion,
        object: &str,
        state: RefState<'_>,
    ) -> Result<GitMutation, AuthorityError> {
        self.converge(self.refs(version).tag, object, state)
    }

    /// Converge the release announcement ref.
    ///
    /// # Errors
    ///
    /// Refuses a noncanonical object or an observed different object.
    pub fn announcement_push(
        &self,
        version: &ProductVersion,
        object: &str,
        state: RefState<'_>,
    ) -> Result<GitMutation, AuthorityError> {
        self.converge(self.refs(version).announcement, object, state)
    }

    fn converge(
        &self,
        reference: String,
        object: &str,
        state: RefState<'_>,
    ) -> Result<GitMutation, AuthorityError> {
        object_id(object)?;
        match state {
            RefState::Absent => Ok(GitMutation::Command(push_command(
                &self.document.remote,
                object,
                &reference,
                Some(None),
                false,
            ))),
            RefState::Present(observed) => {
                object_id(observed)?;
                if observed == object {
                    Ok(GitMutation::Converged)
                } else {
                    Err(AuthorityError::RefMoved {
                        reference,
                        expected: object.to_owned(),
                        observed: observed.to_owned(),
                    })
                }
            }
        }
    }
}

/// Observed state of one immutable release ref.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefState<'a> {
    Absent,
    Present(&'a str),
}

/// Whether one release mutation already converged or needs one command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitMutation {
    Converged,
    Command(GitCommand),
}

/// One exact Git invocation constructed by the release authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitCommand {
    arguments: Vec<String>,
}

impl GitCommand {
    fn new(arguments: impl IntoIterator<Item = String>) -> Self {
        Self {
            arguments: arguments.into_iter().collect(),
        }
    }

    #[must_use]
    pub const fn program(&self) -> &'static str {
        "git"
    }

    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }
}

/// Admit the tracked authority compiled into this crate.
///
/// # Errors
///
/// Refuses the tracked document when it is malformed or noncanonical, or when
/// any workspace crate is externally publishable.
pub fn current_authority(workspace: &Path) -> Result<ReleaseAuthority, AuthorityError> {
    admit_workspace_publish_policy(workspace)?;
    admit_authority(tracked_line(AUTHORITY_BYTES, "release authority")?)
}

/// Admit one exact authority document.
///
/// # Errors
///
/// Refuses malformed or noncanonical JSON and any authority outside the one
/// Git topology this release supports.
pub fn admit_authority(bytes: &[u8]) -> Result<ReleaseAuthority, AuthorityError> {
    let document: AuthorityDocument =
        wire::decode(bytes).map_err(|error| AuthorityError::Decode(error.to_string()))?;
    let canonical =
        wire::encode(&document).map_err(|error| AuthorityError::Encode(error.to_string()))?;
    if canonical != bytes {
        return Err(AuthorityError::NonCanonical);
    }
    fixed(&document)?;
    validate_template(&document.package_ref)?;
    validate_template(&document.tag_ref)?;
    validate_template(&document.announcement_ref)?;
    validate_ref(&document.source)?;
    if !document.source.starts_with("refs/heads/") {
        return Err(AuthorityError::SourceRef(document.source));
    }
    disjoint(&document)?;
    Ok(ReleaseAuthority { document })
}

/// Prove no workspace package can be sent to a Cargo registry.
///
/// # Errors
///
/// Refuses an unreadable or malformed manifest, an externally publishable
/// workspace default, or any member that does not explicitly inherit that
/// default.
pub fn admit_workspace_publish_policy(root: &Path) -> Result<(), AuthorityError> {
    let root_manifest = read_manifest(&root.join("Cargo.toml"))?;
    let workspace = root_manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .ok_or(AuthorityError::WorkspacePublish)?;
    let package = workspace
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or(AuthorityError::WorkspacePublish)?;
    if package.get("publish").and_then(toml::Value::as_bool) != Some(false) {
        return Err(AuthorityError::WorkspacePublish);
    }
    let members = workspace
        .get("members")
        .and_then(toml::Value::as_array)
        .ok_or(AuthorityError::WorkspacePublish)?;
    for member in members {
        let member = member.as_str().ok_or(AuthorityError::WorkspacePublish)?;
        let path = root.join(member).join("Cargo.toml");
        let manifest = read_manifest(&path)?;
        admit_member_publish_policy(&path, &manifest)?;
    }
    Ok(())
}

fn admit_member_publish_policy(path: &Path, manifest: &toml::Value) -> Result<(), AuthorityError> {
    let publish = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("publish"))
        .and_then(toml::Value::as_table)
        .and_then(|publish| publish.get("workspace"))
        .and_then(toml::Value::as_bool);
    if publish == Some(true) {
        Ok(())
    } else {
        Err(AuthorityError::MemberPublish(path.to_path_buf()))
    }
}

fn read_manifest(path: &Path) -> Result<toml::Value, AuthorityError> {
    let bytes = fs::read(path).map_err(|source| AuthorityError::ManifestRead {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_slice(&bytes).map_err(|source| AuthorityError::ManifestDecode {
        path: path.to_path_buf(),
        detail: source.to_string(),
    })
}

fn tracked_line<'a>(bytes: &'a [u8], label: &'static str) -> Result<&'a [u8], AuthorityError> {
    let line = bytes
        .strip_suffix(b"\n")
        .ok_or(AuthorityError::TrackedLine(label))?;
    if line.contains(&b'\n') || line.contains(&b'\r') {
        Err(AuthorityError::TrackedLine(label))
    } else {
        Ok(line)
    }
}

pub(crate) fn tracked_text_line<'a>(
    text: &'a str,
    label: &'static str,
) -> Result<&'a str, AuthorityError> {
    let line = text
        .strip_suffix('\n')
        .ok_or(AuthorityError::TrackedLine(label))?;
    if line.contains('\n') || line.contains('\r') {
        Err(AuthorityError::TrackedLine(label))
    } else {
        Ok(line)
    }
}

fn fixed(document: &AuthorityDocument) -> Result<(), AuthorityError> {
    let fields = [
        (document.schema == AUTHORITY_SCHEMA, "schema"),
        (document.provider == Provider::Git, "provider"),
        (document.package == PACKAGE_NAME, "package"),
        (document.remote == REMOTE, "remote"),
        (document.source == SOURCE_REF, "source"),
        (document.package_ref == PACKAGE_REF, "package_ref"),
        (document.tag_ref == TAG_REF, "tag_ref"),
        (
            document.announcement_ref == ANNOUNCEMENT_REF,
            "announcement_ref",
        ),
        (document.version_file == VERSION_FILE, "version_file"),
    ];
    fields
        .into_iter()
        .find_map(|(matches, field)| (!matches).then_some(field))
        .map_or(Ok(()), |field| Err(AuthorityError::Fixed(field)))
}

fn validate_template(template: &str) -> Result<(), AuthorityError> {
    if template.matches(VERSION_SLOT).count() != 1 {
        return Err(AuthorityError::Template(template.to_owned()));
    }
    validate_ref(&template.replace(VERSION_SLOT, "1.2.3"))
}

fn disjoint(document: &AuthorityDocument) -> Result<(), AuthorityError> {
    let prefixes = [
        template_prefix(&document.package_ref),
        template_prefix(&document.tag_ref),
        template_prefix(&document.announcement_ref),
    ];
    for (index, left) in prefixes.iter().enumerate() {
        for right in &prefixes[index + 1..] {
            if left.starts_with(right) || right.starts_with(left) {
                return Err(AuthorityError::OverlappingRefs);
            }
        }
    }
    Ok(())
}

fn template_prefix(template: &str) -> &str {
    template
        .split_once(VERSION_SLOT)
        .expect("validated template has one slot")
        .0
}

fn expand(template: &str, version: &ProductVersion) -> String {
    template.replace(VERSION_SLOT, version.as_str())
}

fn validate_ref(reference: &str) -> Result<(), AuthorityError> {
    let invalid = !reference.starts_with("refs/")
        || reference.ends_with(['/', '.'])
        || reference.contains("..")
        || reference.contains("@{")
        || reference
            .split('/')
            .any(|part| part.is_empty() || part.ends_with(".lock"))
        || reference.bytes().any(|byte| {
            byte <= b' '
                || byte == ASCII_DELETE
                || matches!(byte, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        });
    if invalid {
        Err(AuthorityError::Reference(reference.to_owned()))
    } else {
        Ok(())
    }
}

fn object_id(object: &str) -> Result<(), AuthorityError> {
    let canonical = matches!(object.len(), 40 | 64)
        && object
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if canonical {
        Ok(())
    } else {
        Err(AuthorityError::Object(object.to_owned()))
    }
}

fn push_command(
    remote: &str,
    object: &str,
    reference: &str,
    lease: Option<Option<&str>>,
    dry_run: bool,
) -> GitCommand {
    let mut arguments = vec!["push".to_owned()];
    if dry_run {
        arguments.push("--dry-run".to_owned());
    }
    arguments.push("--porcelain".to_owned());
    if let Some(expected) = lease {
        arguments.push(format!(
            "--force-with-lease={reference}:{}",
            expected.unwrap_or_default()
        ));
    }
    arguments.push(remote.to_owned());
    arguments.push(format!("{object}:{reference}"));
    GitCommand::new(arguments)
}

fn visibility_command(remote: &str, reference: Option<&str>) -> GitCommand {
    let mut arguments = vec![
        "ls-remote".to_owned(),
        "--refs".to_owned(),
        remote.to_owned(),
    ];
    if let Some(reference) = reference {
        arguments.push(reference.to_owned());
    }
    GitCommand::new(arguments)
}

/// Why the external release authority was refused.
#[derive(Debug, thiserror::Error)]
pub enum AuthorityError {
    #[error("release authority does not decode: {0}")]
    Decode(String),
    #[error("release authority does not encode: {0}")]
    Encode(String),
    #[error("release authority JSON is not canonical")]
    NonCanonical,
    #[error("tracked {0} must be exactly one LF-terminated line")]
    TrackedLine(&'static str),
    #[error("release authority field {0} differs from the fixed Git topology")]
    Fixed(&'static str),
    #[error(transparent)]
    Version(#[from] TrackedVersionError),
    #[error("release ref template must contain VERSION exactly once: {0}")]
    Template(String),
    #[error("release ref is not canonical: {0}")]
    Reference(String),
    #[error("release source ref is not beneath refs/heads: {0}")]
    SourceRef(String),
    #[error("release package, tag, and announcement ref namespaces overlap")]
    OverlappingRefs,
    #[error("Git object is not one canonical lowercase object id: {0}")]
    Object(String),
    #[error("release ref {reference} already names {observed}, not {expected}")]
    RefMoved {
        reference: String,
        expected: String,
        observed: String,
    },
    #[error("workspace package policy is not publish=false")]
    WorkspacePublish,
    #[error("workspace member does not inherit publish=false: {0}")]
    MemberPublish(PathBuf),
    #[error("cannot read Cargo manifest {path}: {source}")]
    ManifestRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot decode Cargo manifest {path}: {detail}")]
    ManifestDecode { path: PathBuf, detail: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJECT: &str = "1111111111111111111111111111111111111111";

    fn workspace() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("dist is one workspace member")
    }

    #[test]
    fn tracked_authority_is_canonical_fixed_and_non_publishable() {
        let authority = current_authority(workspace()).expect("tracked authority admits");
        assert_eq!(authority.package_name(), "spice");
        assert!(tracked_line(AUTHORITY_BYTES, "authority").is_ok());
        assert!(tracked_line(b"{}\n\n", "authority").is_err());
        assert!(tracked_text_line("1\n\n", "version").is_err());
    }

    #[test]
    fn versions_and_ref_templates_have_one_canonical_spelling() {
        for accepted in ["0.0.0", "1.2.3", "1.2.3-rc.1", "1.2.3+build.01"] {
            ProductVersion::parse(accepted).expect("canonical semantic version");
        }
        for refused in ["", "v1.2.3", "01.2.3", "1.2", "1.2.3-01", "1.2.3+"] {
            assert!(ProductVersion::parse(refused).is_err(), "{refused}");
        }

        let authority = current_authority(workspace()).expect("tracked authority");
        let version = ProductVersion::parse("1.2.3").expect("version");
        let refs = authority.refs(&version);
        assert_eq!(
            (
                authority.remote(),
                refs.source(),
                refs.package(),
                refs.tag(),
                refs.announcement(),
            ),
            (
                "origin",
                "refs/heads/main",
                "refs/spice/packages/v1.2.3",
                "refs/tags/v1.2.3",
                "refs/spice/releases/v1.2.3",
            )
        );
    }

    #[test]
    fn rehearsal_visibility_and_convergence_construct_exact_git_operations() {
        let authority = current_authority(workspace()).expect("tracked authority");
        let version = ProductVersion::parse("1.2.3").expect("version");
        assert_eq!(
            authority
                .package_rehearsal(&version, OBJECT)
                .expect("rehearsal")
                .arguments(),
            [
                "push",
                "--dry-run",
                "--porcelain",
                "--force-with-lease=refs/spice/packages/v1.2.3:",
                "origin",
                "1111111111111111111111111111111111111111:refs/spice/packages/v1.2.3",
            ]
        );
        assert_eq!(
            authority.remote_visibility().arguments(),
            ["ls-remote", "--refs", "origin"]
        );
        assert_eq!(authority.remote_visibility().program(), "git");
        assert_eq!(
            authority
                .reference_visibility("refs/tags/v1.2.3")
                .expect("reference visibility")
                .arguments(),
            ["ls-remote", "--refs", "origin", "refs/tags/v1.2.3"]
        );
        assert_eq!(
            authority.package_visibility(&version).arguments(),
            [
                "ls-remote",
                "--refs",
                "origin",
                "refs/spice/packages/v1.2.3"
            ]
        );
        assert_eq!(
            authority
                .package_push(&version, OBJECT, RefState::Present(OBJECT))
                .expect("matching ref converges"),
            GitMutation::Converged
        );
        let error = authority
            .tag_push(
                &version,
                OBJECT,
                RefState::Present("2222222222222222222222222222222222222222"),
            )
            .expect_err("mismatched immutable tag refuses");
        assert!(matches!(error, AuthorityError::RefMoved { .. }));

        let GitMutation::Command(announcement) = authority
            .announcement_push(&version, OBJECT, RefState::Absent)
            .expect("absent announcement can converge")
        else {
            panic!("absent announcement must require one command");
        };
        assert!(
            announcement
                .arguments()
                .iter()
                .any(|argument| argument.ends_with(":refs/spice/releases/v1.2.3"))
        );
        assert!(matches!(
            authority.package_rehearsal(&version, "111111"),
            Err(AuthorityError::Object(_))
        ));
    }

    #[test]
    fn malformed_noncanonical_and_moved_authorities_refuse() {
        let canonical = tracked_line(AUTHORITY_BYTES, "authority").expect("tracked line");
        let mut whitespace = canonical.to_vec();
        whitespace.push(b'\n');
        assert!(matches!(
            admit_authority(&whitespace),
            Err(AuthorityError::NonCanonical)
        ));

        let moved = String::from_utf8(canonical.to_vec())
            .expect("fixture JSON")
            .replace("refs/heads/main", "refs/heads/release");
        assert!(matches!(
            admit_authority(moved.as_bytes()),
            Err(AuthorityError::Fixed("source"))
        ));

        let mut unknown: serde_json::Value =
            serde_json::from_slice(canonical).expect("fixture authority");
        unknown["ambient_remote_url"] = serde_json::Value::Bool(true);
        assert!(matches!(
            admit_authority(&serde_json::to_vec(&unknown).expect("changed authority")),
            Err(AuthorityError::Decode(_))
        ));
    }

    #[test]
    fn ref_grammar_templates_and_namespaces_refuse_independently() {
        for reference in [
            "main",
            "refs/heads/release candidate",
            "refs/heads/release..candidate",
            "refs/heads//release",
            "refs/heads/release~candidate",
            "refs/heads/candidate.lock",
        ] {
            assert!(validate_ref(reference).is_err(), "{reference}");
        }
        for template in [
            "refs/spice/packages/current",
            "refs/spice/VERSION/packages/VERSION",
        ] {
            assert!(validate_template(template).is_err(), "{template}");
        }

        let mut document: AuthorityDocument = wire::decode(
            tracked_line(AUTHORITY_BYTES, "authority").expect("tracked authority line"),
        )
        .expect("fixture authority");
        document.announcement_ref.clone_from(&document.package_ref);
        assert!(matches!(
            disjoint(&document),
            Err(AuthorityError::OverlappingRefs)
        ));
        document.announcement_ref = "refs/spice/packages/vnested/VERSION".to_owned();
        assert!(matches!(
            disjoint(&document),
            Err(AuthorityError::OverlappingRefs)
        ));
    }

    #[test]
    fn a_publishable_member_refuses_the_workspace_policy() {
        let publishable: toml::Value = toml::from_str(
            r#"
                [package]
                name = "public-by-default"
                version = "1.0.0"
            "#,
        )
        .expect("fixture manifest");
        assert!(matches!(
            admit_member_publish_policy(Path::new("public/Cargo.toml"), &publishable),
            Err(AuthorityError::MemberPublish(path)) if path == Path::new("public/Cargo.toml")
        ));
    }
}
