use super::*;

pub(super) struct FocusedDispatch {
    pub(super) body: GateFindings,
    pub(super) findings: Vec<FindingRow>,
    pub(super) repository: Repository,
}

pub(super) fn focused_dispatch(
    root: &Path,
    marker: &str,
    finding_names: &[&str],
) -> FocusedDispatch {
    focused_dispatch_in(root, "focused-repository", marker, finding_names)
}

pub(super) fn focused_dispatch_in(
    root: &Path,
    repository_name: &str,
    marker: &str,
    finding_names: &[&str],
) -> FocusedDispatch {
    let repository = focused_repository(root, repository_name);
    let repository_identity = digest(&format!("repository:{repository_name}"));
    let tree = digest(&format!("tree:{marker}"));
    let evidence = format!("in-process focused report {marker}");
    FocusedDispatch {
        body: GateFindings {
            report: Digest::compute(Domain::GateFocusedReport, evidence.as_bytes()),
            evidence,
            source: wire::intent::GateFindingSource {
                repository: repository_identity,
                selection: digest(&format!("selection:{marker}")),
                tree,
                anchor: format!("anchor-{marker}"),
                kind: wire::intent::GateFindingSourceKind::StagedIndex,
                mainline: Some(wire::intent::GateFindingMainline {
                    reference: "refs/remotes/origin/main".to_owned(),
                    merge_base: format!("mainline-base-{marker}"),
                }),
            },
        },
        findings: finding_names.iter().map(|name| finding(name)).collect(),
        repository,
    }
}

fn focused_repository(root: &Path, name: &str) -> Repository {
    let repository = root.join(name);
    fs::create_dir_all(repository.join(".git"))
        .expect("focused in-process repository authority exists");
    Repository::discover(&repository).expect("discover focused in-process repository")
}
