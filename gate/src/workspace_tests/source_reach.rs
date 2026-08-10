/// Acquire exact external module and `include!` sources with their Cargo target.
fn declared_path_sources(target_source: &Path) -> Result<Vec<PathBuf>, WorkspaceTestError> {
    let module_dir = target_source.parent().ok_or_else(|| {
        WorkspaceTestError::new(format!(
            "Cargo target source {} has no parent",
            target_source.display()
        ))
    })?;
    let mut declared = BTreeSet::new();
    let mut visited = BTreeSet::new();
    collect_declared_sources(target_source, module_dir, &mut declared, &mut visited)?;
    declared.remove(target_source);
    Ok(declared.into_iter().collect())
}

fn collect_declared_sources(
    source: &Path,
    module_dir: &Path,
    declared: &mut BTreeSet<PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<(), WorkspaceTestError> {
    let source = fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    if !visited.insert(source.clone()) {
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
    collect_declared_items(&syntax.items, &source, module_dir, declared, visited)
}

fn collect_declared_items(
    items: &[Item],
    source: &Path,
    module_dir: &Path,
    declared: &mut BTreeSet<PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<(), WorkspaceTestError> {
    for item in items {
        match item {
            Item::Mod(module) => {
                if let Some((_, inline)) = &module.content {
                    let child_dir = module_dir.join(module.ident.to_string());
                    collect_declared_items(inline, source, &child_dir, declared, visited)?;
                } else if let Some(module_source) =
                    external_module_source(module, source, module_dir)?
                {
                    let module_source = fs::canonicalize(&module_source).unwrap_or(module_source);
                    let child_dir = child_module_dir(&module_source)?;
                    declared.insert(module_source.clone());
                    collect_declared_sources(&module_source, &child_dir, declared, visited)?;
                }
            }
            Item::Macro(invocation) if invocation.mac.path.is_ident("include") => {
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
                declared.insert(included.clone());
                collect_declared_sources(&included, module_dir, declared, visited)?;
            }
            _ => {}
        }
    }
    Ok(())
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
