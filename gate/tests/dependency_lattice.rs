use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const EXPECTED: &[(&str, &[&str])] = &[
    ("app", &["wire"]),
    ("atomic", &[]),
    ("board", &["dialect", "fold", "journal", "wire"]),
    ("bus", &["fold", "journal"]),
    ("canvas", &["atomic", "bus", "fold", "journal", "wire"]),
    ("consent", &["fold", "journal", "wire"]),
    ("dialect", &[]),
    ("dist", &["atomic", "sill", "wire"]),
    ("fold", &["journal", "wire"]),
    ("gate", &["atomic", "sill", "wire"]),
    ("journal", &["atomic", "sill", "wire"]),
    (
        "lane",
        &[
            "atomic", "bus", "consent", "fold", "journal", "listen", "sill", "steer", "tape",
            "taste", "vendor",
        ],
    ),
    ("listen", &["tape", "wire"]),
    ("proof", &["atomic", "gate", "sill", "wire"]),
    ("sill", &[]),
    (
        "spice",
        &[
            "atomic", "board", "bus", "canvas", "consent", "dialect", "fold", "gate", "journal",
            "lane", "listen", "proof", "sill", "steer", "tape", "taste", "vendor", "wire",
        ],
    ),
    ("steer", &["atomic", "fold", "journal", "sill", "wire"]),
    ("tape", &["atomic", "sill"]),
    ("taste", &["fold", "journal", "listen", "wire"]),
    ("vendor", &["listen", "sill", "wire"]),
    ("wire", &[]),
    ("xtask", &[]),
];

#[test]
fn manifests_match_the_documented_dependency_lattice() {
    let root = workspace_root();
    let members = workspace_members(&root);
    let expected = expected_lattice();

    assert_eq!(members, expected.keys().cloned().collect());
    let actual = locked_lattice(&root, &members);

    assert_acyclic(&actual);
    assert_eq!(actual, expected);
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("gate must be a direct workspace member")
        .to_path_buf()
}

fn workspace_members(root: &Path) -> BTreeSet<String> {
    let manifest =
        fs::read_to_string(root.join("Cargo.toml")).expect("workspace Cargo.toml must be readable");
    let mut members = BTreeSet::new();
    let mut reading_members = false;

    for line in manifest.lines().map(str::trim) {
        if line.starts_with("members") && line.contains('[') {
            reading_members = true;
            continue;
        }
        if reading_members && line.starts_with(']') {
            break;
        }
        if reading_members {
            let member = line.trim_end_matches(',').trim_matches('"');
            assert!(
                members.insert(member.to_owned()),
                "duplicate member {member}"
            );
        }
    }

    assert!(!members.is_empty(), "workspace members were not found");
    members
}

fn expected_lattice() -> BTreeMap<String, BTreeSet<String>> {
    EXPECTED
        .iter()
        .map(|(name, dependencies)| {
            (
                (*name).to_owned(),
                dependencies.iter().map(|name| (*name).to_owned()).collect(),
            )
        })
        .collect()
}

#[derive(Default)]
struct LockedPackage {
    name: String,
    version: String,
    source: Option<String>,
    dependencies: Vec<String>,
}

fn locked_lattice(root: &Path, members: &BTreeSet<String>) -> BTreeMap<String, BTreeSet<String>> {
    let lockfile =
        fs::read_to_string(root.join("Cargo.lock")).expect("workspace Cargo.lock must be readable");
    let packages = lockfile
        .split("[[package]]")
        .skip(1)
        .map(parse_locked_package)
        .collect::<Vec<_>>();
    let mut workspace_versions = BTreeMap::new();

    for package in &packages {
        if package.source.is_none() && members.contains(&package.name) {
            assert!(
                workspace_versions
                    .insert(package.name.clone(), package.version.clone())
                    .is_none(),
                "Cargo.lock contains duplicate workspace package {}",
                package.name
            );
        }
    }
    assert_eq!(
        workspace_versions.keys().cloned().collect::<BTreeSet<_>>(),
        *members
    );

    packages
        .iter()
        .filter(|package| package.source.is_none() && members.contains(&package.name))
        .map(|package| {
            let dependencies = package
                .dependencies
                .iter()
                .filter_map(|dependency| {
                    workspace_versions.iter().find_map(|(name, version)| {
                        let qualified = format!("{name} {version}");
                        (dependency == name || dependency == &qualified).then(|| name.clone())
                    })
                })
                .collect();
            (package.name.clone(), dependencies)
        })
        .collect()
}

fn parse_locked_package(block: &str) -> LockedPackage {
    let mut package = LockedPackage::default();
    let mut reading_dependencies = false;

    for line in block.lines().map(str::trim) {
        if line == "dependencies = [" {
            reading_dependencies = true;
            continue;
        }
        if reading_dependencies && line == "]" {
            reading_dependencies = false;
            continue;
        }
        if reading_dependencies {
            package
                .dependencies
                .push(line.trim_end_matches(',').trim_matches('"').to_owned());
        } else if let Some(value) = quoted_assignment(line, "name") {
            package.name = value.to_owned();
        } else if let Some(value) = quoted_assignment(line, "version") {
            package.version = value.to_owned();
        } else if let Some(value) = quoted_assignment(line, "source") {
            package.source = Some(value.to_owned());
        }
    }

    assert!(!package.name.is_empty(), "lockfile package has no name");
    assert!(
        !package.version.is_empty(),
        "lockfile package has no version"
    );
    package
}

fn quoted_assignment<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let value = line
        .strip_prefix(key)?
        .trim_start()
        .strip_prefix('=')?
        .trim_start()
        .strip_prefix('"')?;
    value.strip_suffix('"')
}

fn assert_acyclic(lattice: &BTreeMap<String, BTreeSet<String>>) {
    fn visit<'a>(
        crate_name: &'a str,
        lattice: &'a BTreeMap<String, BTreeSet<String>>,
        active: &mut BTreeSet<&'a str>,
        complete: &mut BTreeSet<&'a str>,
    ) {
        if complete.contains(crate_name) {
            return;
        }
        assert!(
            active.insert(crate_name),
            "dependency lattice contains a cycle through {crate_name}"
        );

        for dependency in &lattice[crate_name] {
            visit(dependency, lattice, active, complete);
        }

        active.remove(crate_name);
        complete.insert(crate_name);
    }

    let mut active = BTreeSet::new();
    let mut complete = BTreeSet::new();
    for crate_name in lattice.keys() {
        visit(crate_name, lattice, &mut active, &mut complete);
    }
}
