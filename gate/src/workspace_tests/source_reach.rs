use super::*;

#[derive(Clone, Copy)]
struct DeclaredReach {
    test_only: bool,
    dependent_tests: bool,
}

#[derive(Default)]
struct SourceGraph {
    declared: BTreeMap<PathBuf, DeclaredReach>,
    visited: BTreeSet<(PathBuf, bool, bool, Vec<Vec<String>>)>,
    conservative_downstream: bool,
}

/// Acquire exact external module and `include!` sources with their Cargo target.
pub(super) fn declared_path_sources(
    target_source: &Path,
) -> Result<Vec<DeclaredPathSource>, WorkspaceTestError> {
    let module_dir = target_source.parent().ok_or_else(|| {
        WorkspaceTestError::new(format!(
            "Cargo target source {} has no parent",
            target_source.display()
        ))
    })?;
    let mut graph = SourceGraph::default();
    graph.collect_declared_sources(
        target_source,
        module_dir,
        DeclaredReach {
            test_only: false,
            dependent_tests: true,
        },
        &[],
        0,
    )?;
    if graph.conservative_downstream {
        for reach in graph.declared.values_mut() {
            if !reach.test_only {
                reach.dependent_tests = true;
            }
        }
    }
    graph.declared.remove(target_source);
    Ok(graph
        .declared
        .into_iter()
        .map(|(path, reach)| DeclaredPathSource {
            path,
            test_only: reach.test_only,
            dependent_tests: reach.dependent_tests,
        })
        .collect())
}

impl SourceGraph {
    fn collect_declared_sources(
        &mut self,
        source: &Path,
        module_dir: &Path,
        reach: DeclaredReach,
        inherited_reexports: &[Vec<String>],
        module_depth: usize,
    ) -> Result<(), WorkspaceTestError> {
        let source = fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
        if !self.visited.insert((
            source.clone(),
            reach.test_only,
            reach.dependent_tests,
            inherited_reexports.to_vec(),
        )) {
            return Ok(());
        }
        let document = match fs::read_to_string(&source) {
            Ok(document) => document,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(WorkspaceTestError::new(format!(
                    "read Rust source {} while resolving staged test reach: {error}",
                    source.display()
                )));
            }
        };
        let syntax = syn::parse_file(&document).map_err(|error| {
            WorkspaceTestError::new(format!(
                "parse Rust source {} while resolving staged test reach: {error}",
                source.display()
            ))
        })?;
        self.collect_declared_items(
            &syntax.items,
            &source,
            module_dir,
            reach,
            inherited_reexports,
            module_depth,
        )
    }

    fn collect_declared_items(
        &mut self,
        items: &[Item],
        source: &Path,
        module_dir: &Path,
        reach: DeclaredReach,
        inherited_reexports: &[Vec<String>],
        module_depth: usize,
    ) -> Result<(), WorkspaceTestError> {
        let (mut reexport_paths, uncertain_reexport) = public_reexport_paths(items, module_depth);
        reexport_paths.extend(inherited_reexports.iter().cloned());
        if reach.dependent_tests && uncertain_reexport {
            self.conservative_downstream = true;
        }
        for item in items {
            match item {
                Item::Mod(module) => {
                    let test_only = reach.test_only || exact_cfg_test(&module.attrs);
                    let module_name = module.ident.to_string();
                    let child_reexports = reexport_paths
                        .iter()
                        .filter(|path| path.first().is_some_and(|segment| segment == &module_name))
                        .filter(|path| path.len() > 1)
                        .map(|path| path[1..].to_vec())
                        .collect::<Vec<_>>();
                    let reexported = reexport_paths
                        .iter()
                        .any(|path| path.first().is_some_and(|segment| segment == &module_name));
                    let child_reach = DeclaredReach {
                        test_only,
                        dependent_tests: !test_only
                            && reach.dependent_tests
                            && (public_visibility(&module.vis) || reexported || uncertain_reexport),
                    };
                    if let Some((_, inline)) = &module.content {
                        let child_dir = module_dir.join(module_name);
                        self.collect_declared_items(
                            inline,
                            source,
                            &child_dir,
                            child_reach,
                            &child_reexports,
                            module_depth.saturating_add(1),
                        )?;
                    } else if let Some(module_source) =
                        external_module_source(module, source, module_dir)?
                    {
                        let module_source =
                            fs::canonicalize(&module_source).unwrap_or(module_source);
                        let child_dir = child_module_dir(&module_source)?;
                        record_declared_source(
                            &mut self.declared,
                            module_source.clone(),
                            child_reach.test_only,
                            child_reach.dependent_tests,
                        );
                        self.collect_declared_sources(
                            &module_source,
                            &child_dir,
                            child_reach,
                            &child_reexports,
                            module_depth.saturating_add(1),
                        )?;
                    }
                }
                Item::Macro(invocation) if invocation.mac.path.is_ident("include") => {
                    let test_only = reach.test_only || exact_cfg_test(&invocation.attrs);
                    let included_reach = DeclaredReach {
                        test_only,
                        dependent_tests: !test_only && reach.dependent_tests,
                    };
                    let path =
                        syn::parse2::<LitStr>(invocation.mac.tokens.clone()).map_err(|error| {
                            WorkspaceTestError::new(format!(
                                "Rust source {} has a non-literal include path: {error}",
                                source.display()
                            ))
                        })?;
                    let parent = source.parent().ok_or_else(|| {
                        WorkspaceTestError::new(format!(
                            "included Rust source {} has no parent",
                            source.display()
                        ))
                    })?;
                    let included = parent.join(path.value());
                    let included = fs::canonicalize(&included).unwrap_or(included);
                    record_declared_source(
                        &mut self.declared,
                        included.clone(),
                        included_reach.test_only,
                        included_reach.dependent_tests,
                    );
                    self.collect_declared_sources(
                        &included,
                        module_dir,
                        included_reach,
                        &reexport_paths,
                        module_depth,
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn public_visibility(visibility: &syn::Visibility) -> bool {
    matches!(visibility, syn::Visibility::Public(_))
}

fn public_reexport_paths(items: &[Item], module_depth: usize) -> (Vec<Vec<String>>, bool) {
    let mut paths = Vec::new();
    let mut uncertain = false;
    for item in items {
        let Item::Use(item) = item else {
            continue;
        };
        if !public_visibility(&item.vis) || exact_cfg_test(&item.attrs) {
            continue;
        }
        collect_public_use_paths(
            &item.tree,
            module_depth,
            Vec::new(),
            &mut paths,
            &mut uncertain,
        );
    }
    (paths, uncertain)
}

fn collect_public_use_paths(
    tree: &syn::UseTree,
    module_depth: usize,
    prefix: Vec<String>,
    paths: &mut Vec<Vec<String>>,
    uncertain: &mut bool,
) {
    match tree {
        syn::UseTree::Path(path) if path.ident == "self" => {
            collect_public_use_paths(&path.tree, module_depth, prefix, paths, uncertain);
        }
        syn::UseTree::Path(path) if path.ident == "crate" && module_depth == 0 => {
            collect_public_use_paths(&path.tree, module_depth, prefix, paths, uncertain);
        }
        syn::UseTree::Path(path) if path.ident == "crate" || path.ident == "super" => {
            *uncertain = true;
        }
        syn::UseTree::Path(path) => {
            let mut prefix = prefix;
            prefix.push(path.ident.to_string());
            collect_public_use_paths(&path.tree, module_depth, prefix, paths, uncertain);
        }
        syn::UseTree::Name(name) => {
            let mut path = prefix;
            path.push(name.ident.to_string());
            paths.push(path);
        }
        syn::UseTree::Rename(rename) => {
            let mut path = prefix;
            path.push(rename.ident.to_string());
            paths.push(path);
        }
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                collect_public_use_paths(tree, module_depth, prefix.clone(), paths, uncertain);
            }
        }
        syn::UseTree::Glob(_) => *uncertain = true,
    }
}

fn exact_cfg_test(attributes: &[syn::Attribute]) -> bool {
    // Compound cfg expressions may admit a non-test build, so only an exact
    // `#[cfg(test)]` edge can prove that every descendant is test-only.
    attributes.iter().any(|attribute| {
        if !attribute.path().is_ident("cfg") {
            return false;
        }
        let Meta::List(list) = &attribute.meta else {
            return false;
        };
        syn::parse2::<syn::Path>(list.tokens.clone()).is_ok_and(|path| path.is_ident("test"))
    })
}

fn record_declared_source(
    declared: &mut BTreeMap<PathBuf, DeclaredReach>,
    source: PathBuf,
    test_only: bool,
    dependent_tests: bool,
) {
    // One production edge is enough to make a multiply-declared source part
    // of ordinary library reach; test-only survives only when every edge is.
    declared
        .entry(source)
        .and_modify(|existing| {
            existing.test_only &= test_only;
            existing.dependent_tests |= dependent_tests;
        })
        .or_insert(DeclaredReach {
            test_only,
            dependent_tests,
        });
}

fn child_module_dir(source: &Path) -> Result<PathBuf, WorkspaceTestError> {
    let parent = source.parent().ok_or_else(|| {
        WorkspaceTestError::new(format!(
            "Rust module source {} has no parent",
            source.display()
        ))
    })?;
    if source.file_name() == Some(OsStr::new("mod.rs")) {
        Ok(parent.to_path_buf())
    } else {
        let stem = source.file_stem().ok_or_else(|| {
            WorkspaceTestError::new(format!(
                "Rust module source {} has no stem",
                source.display()
            ))
        })?;
        Ok(parent.join(stem))
    }
}

fn external_module_source(
    module: &syn::ItemMod,
    source: &Path,
    module_dir: &Path,
) -> Result<Option<PathBuf>, WorkspaceTestError> {
    let paths = module
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("path"))
        .map(|attribute| path_attribute(source, &attribute.meta))
        .collect::<Result<Vec<_>, _>>()?;
    if paths.len() > 1 {
        return Err(WorkspaceTestError::new(format!(
            "Rust source {} declares multiple paths for module {}",
            source.display(),
            module.ident
        )));
    }
    if let Some(path) = paths.into_iter().next() {
        let parent = source.parent().ok_or_else(|| {
            WorkspaceTestError::new(format!("Rust source {} has no parent", source.display()))
        })?;
        return Ok(Some(parent.join(path)));
    }
    let name = module.ident.to_string();
    let candidates = [
        module_dir.join(format!("{name}.rs")),
        module_dir.join(&name).join("mod.rs"),
    ];
    Ok(candidates.into_iter().find(|candidate| candidate.is_file()))
}

fn path_attribute(source: &Path, meta: &Meta) -> Result<String, WorkspaceTestError> {
    let Meta::NameValue(pair) = meta else {
        return Err(WorkspaceTestError::new(format!(
            "Rust source {} has a non-value path attribute",
            source.display()
        )));
    };
    let Expr::Lit(expression) = &pair.value else {
        return Err(WorkspaceTestError::new(format!(
            "Rust source {} has a non-literal path attribute",
            source.display()
        )));
    };
    let Lit::Str(path) = &expression.lit else {
        return Err(WorkspaceTestError::new(format!(
            "Rust source {} has a non-string path attribute",
            source.display()
        )));
    };
    Ok(path.value())
}

#[cfg(test)]
mod tests;
