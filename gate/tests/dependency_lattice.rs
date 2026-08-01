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
];

#[test]
fn manifests_match_the_documented_dependency_lattice() {
    let root = workspace_root();
    let members = workspace_members(&root);
    let expected = expected_lattice();

    assert_eq!(members, expected.keys().cloned().collect());

    let member_paths = members
        .iter()
        .map(|member| {
            let path = fs::canonicalize(root.join(member)).unwrap_or_else(|error| {
                panic!("failed to resolve workspace member {member}: {error}")
            });
            (path, member.clone())
        })
        .collect::<BTreeMap<_, _>>();

    let actual = members
        .iter()
        .map(|member| {
            (
                member.clone(),
                local_dependencies(&root, member, &member_paths),
            )
        })
        .collect::<BTreeMap<_, _>>();

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

fn local_dependencies(
    root: &Path,
    member: &str,
    member_paths: &BTreeMap<PathBuf, String>,
) -> BTreeSet<String> {
    let manifest_path = root.join(member).join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));
    let mut dependencies = BTreeSet::new();

    for (index, line) in manifest.lines().enumerate() {
        let Some(relative_path) = dependency_path(line) else {
            continue;
        };
        let dependency_path = fs::canonicalize(root.join(member).join(relative_path))
            .unwrap_or_else(|error| {
                panic!(
                    "failed to resolve path dependency in {}:{}: {error}",
                    manifest_path.display(),
                    index + 1
                )
            });
        let Some(dependency) = member_paths.get(&dependency_path) else {
            continue;
        };

        assert!(
            dependencies.insert(dependency.clone()),
            "{} declares workspace member {dependency} more than once",
            manifest_path.display()
        );
    }

    dependencies
}

fn dependency_path(line: &str) -> Option<&str> {
    let (_, after_path) = line.split_once("path")?;
    let after_path = after_path.trim_start().strip_prefix('=')?.trim_start();
    let after_quote = after_path.strip_prefix('"')?;
    let (path, _) = after_quote.split_once('"')?;
    Some(path)
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
