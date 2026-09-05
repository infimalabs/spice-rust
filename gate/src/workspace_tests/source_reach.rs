use super::*;
use proc_macro2::{TokenStream, TokenTree};
use syn::parse::{Parse, ParseStream};
use syn::visit::Visit;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct DeclaredReach {
    test_only: bool,
    dependent_tests: bool,
    public_namespace: bool,
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct SourceVisit {
    source: PathBuf,
    reach: DeclaredReach,
    inherited_reexports: Vec<Vec<String>>,
}

struct PublicUse {
    path: Vec<String>,
    exported_name: String,
}

struct SourceGraph {
    selected_root: PathBuf,
    declared: BTreeMap<PathBuf, DeclaredReach>,
    visited: BTreeSet<SourceVisit>,
    conservative_downstream: bool,
}

#[derive(Clone, Copy)]
enum FileMacroKind {
    Include,
    IncludeStr,
    IncludeBytes,
}

impl FileMacroKind {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "include" => Some(Self::Include),
            "include_str" => Some(Self::IncludeStr),
            "include_bytes" => Some(Self::IncludeBytes),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Include => "include!",
            Self::IncludeStr => "include_str!",
            Self::IncludeBytes => "include_bytes!",
        }
    }
}

struct FileMacroReference {
    kind: FileMacroKind,
    path: String,
}

struct FileMacroArgument {
    path: syn::LitStr,
}

impl Parse for FileMacroArgument {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let path = input.parse()?;
        let _trailing_comma = input.parse::<Option<syn::Token![,]>>()?;
        Ok(Self { path })
    }
}

struct FileMacroVisitor<'a> {
    source: &'a Path,
    references: Vec<FileMacroReference>,
    error: Option<WorkspaceTestError>,
}

/// Acquire exact external module and built-in file-macro sources with their Cargo target.
pub(super) fn declared_path_sources(
    selected_root: &Path,
    target_source: &Path,
) -> Result<Vec<DeclaredPathSource>, WorkspaceTestError> {
    let selected_root =
        fs::canonicalize(selected_root).unwrap_or_else(|_| selected_root.to_path_buf());
    let module_dir = target_source.parent().ok_or_else(|| {
        WorkspaceTestError::new(format!(
            "Cargo target source {} has no parent",
            target_source.display()
        ))
    })?;
    let mut graph = SourceGraph {
        selected_root,
        declared: BTreeMap::new(),
        visited: BTreeSet::new(),
        conservative_downstream: false,
    };
    graph.collect_declared_sources(
        target_source,
        module_dir,
        DeclaredReach {
            test_only: false,
            dependent_tests: true,
            public_namespace: true,
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
        if !self.visited.insert(SourceVisit {
            source: source.clone(),
            reach,
            inherited_reexports: inherited_reexports.to_vec(),
        }) {
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
        let (local_reexports, uncertain_reexport) = public_reexports(items, module_depth);
        if reach.dependent_tests && uncertain_reexport {
            self.conservative_downstream = true;
        }
        let requested_names = inherited_reexports
            .iter()
            .filter_map(|path| path.first())
            .collect::<BTreeSet<_>>();
        let mut reexport_paths = inherited_reexports.to_vec();
        reexport_paths.extend(
            local_reexports
                .into_iter()
                .filter(|reexport| {
                    reach.public_namespace
                        || (reach.dependent_tests
                            && requested_names.contains(&reexport.exported_name))
                })
                .map(|reexport| reexport.path),
        );
        for item in items {
            let test_only = reach.test_only || exact_cfg_test(item_attributes(item));
            let item_reach = DeclaredReach {
                test_only,
                dependent_tests: !test_only && reach.dependent_tests,
                public_namespace: !test_only && reach.public_namespace,
            };
            for reference in file_macro_references(item, source)? {
                self.collect_file_macro_source(
                    reference,
                    source,
                    module_dir,
                    item_reach,
                    &reexport_paths,
                    module_depth,
                )?;
            }
            if let Item::Mod(module) = item {
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
                let reexported_namespace = reexport_paths.iter().any(|path| {
                    path.len() == 1 && path.first().is_some_and(|segment| segment == &module_name)
                });
                let directly_public = reach.public_namespace && public_visibility(&module.vis);
                let child_reach = DeclaredReach {
                    test_only: item_reach.test_only,
                    dependent_tests: !item_reach.test_only && (directly_public || reexported),
                    public_namespace: !item_reach.test_only
                        && (directly_public || reexported_namespace),
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
                    let module_source = fs::canonicalize(&module_source).unwrap_or(module_source);
                    let child_dir = child_module_dir(&module_source)?;
                    record_declared_source(&mut self.declared, module_source.clone(), child_reach);
                    self.collect_declared_sources(
                        &module_source,
                        &child_dir,
                        child_reach,
                        &child_reexports,
                        module_depth.saturating_add(1),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn collect_file_macro_source(
        &mut self,
        reference: FileMacroReference,
        source: &Path,
        module_dir: &Path,
        reach: DeclaredReach,
        inherited_reexports: &[Vec<String>],
        module_depth: usize,
    ) -> Result<(), WorkspaceTestError> {
        let included = self.admit_file_macro_source(source, &reference)?;
        record_declared_source(&mut self.declared, included.clone(), reach);
        if matches!(reference.kind, FileMacroKind::Include) {
            self.collect_included_rust_source(
                &included,
                module_dir,
                reach,
                inherited_reexports,
                module_depth,
            )?;
        }
        Ok(())
    }

    fn admit_file_macro_source(
        &self,
        source: &Path,
        reference: &FileMacroReference,
    ) -> Result<PathBuf, WorkspaceTestError> {
        let parent = source.parent().ok_or_else(|| {
            WorkspaceTestError::new(format!("Rust source {} has no parent", source.display()))
        })?;
        let candidate = parent.join(&reference.path);
        let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
            WorkspaceTestError::new(format!(
                "Rust source {} {} input {} is not readable selected-tree authority: {error}",
                source.display(),
                reference.kind.name(),
                candidate.display(),
            ))
        })?;
        if !metadata.file_type().is_file() {
            return Err(WorkspaceTestError::new(format!(
                "Rust source {} {} input {} is not a regular file",
                source.display(),
                reference.kind.name(),
                candidate.display(),
            )));
        }
        let included = fs::canonicalize(&candidate).map_err(|error| {
            WorkspaceTestError::new(format!(
                "canonicalize Rust source {} {} input {}: {error}",
                source.display(),
                reference.kind.name(),
                candidate.display(),
            ))
        })?;
        if !included.starts_with(&self.selected_root) {
            return Err(WorkspaceTestError::new(format!(
                "Rust source {} {} input {} escapes selected root {}",
                source.display(),
                reference.kind.name(),
                included.display(),
                self.selected_root.display(),
            )));
        }
        if included.to_str().is_none() {
            return Err(WorkspaceTestError::new(format!(
                "Rust source {} {} input path {} is not UTF-8",
                source.display(),
                reference.kind.name(),
                included.display(),
            )));
        }
        let bytes = fs::read(&included).map_err(|error| {
            WorkspaceTestError::new(format!(
                "read Rust source {} {} input {}: {error}",
                source.display(),
                reference.kind.name(),
                included.display(),
            ))
        })?;
        if matches!(
            reference.kind,
            FileMacroKind::Include | FileMacroKind::IncludeStr
        ) && std::str::from_utf8(&bytes).is_err()
        {
            return Err(WorkspaceTestError::new(format!(
                "Rust source {} {} input {} is not UTF-8",
                source.display(),
                reference.kind.name(),
                included.display(),
            )));
        }
        Ok(included)
    }

    fn collect_included_rust_source(
        &mut self,
        source: &Path,
        module_dir: &Path,
        reach: DeclaredReach,
        inherited_reexports: &[Vec<String>],
        module_depth: usize,
    ) -> Result<(), WorkspaceTestError> {
        let visit = SourceVisit {
            source: source.to_path_buf(),
            reach,
            inherited_reexports: inherited_reexports.to_vec(),
        };
        if !self.visited.insert(visit) {
            return Ok(());
        }
        let document = fs::read_to_string(source).map_err(|error| {
            WorkspaceTestError::new(format!(
                "read included Rust source {} while resolving staged test reach: {error}",
                source.display()
            ))
        })?;
        match syn::parse_file(&document) {
            Ok(syntax) => self.collect_declared_items(
                &syntax.items,
                source,
                module_dir,
                reach,
                inherited_reexports,
                module_depth,
            ),
            Err(file_error) => {
                let tokens = document.parse::<TokenStream>().map_err(|token_error| {
                    WorkspaceTestError::new(format!(
                        "parse included Rust source {} while resolving staged test reach: {file_error}; tokenization also failed: {token_error}",
                        source.display()
                    ))
                })?;
                for reference in file_macro_references_in_tokens(tokens, source)? {
                    self.collect_file_macro_source(
                        reference,
                        source,
                        module_dir,
                        reach,
                        inherited_reexports,
                        module_depth,
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl<'ast> Visit<'ast> for FileMacroVisitor<'_> {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        for attribute in &item.attrs {
            self.visit_attribute(attribute);
        }
    }

    fn visit_macro(&mut self, invocation: &'ast syn::Macro) {
        if let Some(kind) = invocation
            .path
            .get_ident()
            .and_then(|ident| FileMacroKind::from_name(&ident.to_string()))
        {
            self.record(kind, invocation.tokens.clone());
        } else {
            self.scan(invocation.tokens.clone());
        }
    }

    fn visit_token_stream(&mut self, tokens: &'ast TokenStream) {
        self.scan(tokens.clone());
    }
}

impl FileMacroVisitor<'_> {
    fn record(&mut self, kind: FileMacroKind, tokens: TokenStream) {
        if self.error.is_some() {
            return;
        }
        match syn::parse2::<FileMacroArgument>(tokens) {
            Ok(argument) => self.references.push(FileMacroReference {
                kind,
                path: argument.path.value(),
            }),
            Err(error) => {
                self.error = Some(WorkspaceTestError::new(format!(
                    "Rust source {} has a non-literal {} path: {error}",
                    self.source.display(),
                    kind.name(),
                )));
            }
        }
    }

    fn scan(&mut self, tokens: TokenStream) {
        if self.error.is_some() {
            return;
        }
        match file_macro_references_in_tokens(tokens, self.source) {
            Ok(references) => self.references.extend(references),
            Err(error) => self.error = Some(error),
        }
    }
}

fn file_macro_references(
    item: &Item,
    source: &Path,
) -> Result<Vec<FileMacroReference>, WorkspaceTestError> {
    let mut visitor = FileMacroVisitor {
        source,
        references: Vec::new(),
        error: None,
    };
    visitor.visit_item(item);
    match visitor.error {
        Some(error) => Err(error),
        None => Ok(visitor.references),
    }
}

fn file_macro_references_in_tokens(
    tokens: TokenStream,
    source: &Path,
) -> Result<Vec<FileMacroReference>, WorkspaceTestError> {
    let trees = tokens.into_iter().collect::<Vec<_>>();
    let mut references = Vec::new();
    let mut index = 0;
    while index < trees.len() {
        let TokenTree::Ident(ident) = &trees[index] else {
            if let TokenTree::Group(group) = &trees[index] {
                references.extend(file_macro_references_in_tokens(group.stream(), source)?);
            }
            index += 1;
            continue;
        };
        let Some(kind) = FileMacroKind::from_name(&ident.to_string()) else {
            index += 1;
            continue;
        };
        let namespaced = index >= 2
            && matches!(&trees[index - 1], TokenTree::Punct(punct) if punct.as_char() == ':')
            && matches!(&trees[index - 2], TokenTree::Punct(punct) if punct.as_char() == ':');
        let invocation = matches!(trees.get(index + 1), Some(TokenTree::Punct(punct)) if punct.as_char() == '!')
            && matches!(trees.get(index + 2), Some(TokenTree::Group(_)));
        if namespaced || !invocation {
            index += 1;
            continue;
        }
        let Some(TokenTree::Group(arguments)) = trees.get(index + 2) else {
            unreachable!("macro invocation shape was checked")
        };
        let argument = syn::parse2::<FileMacroArgument>(arguments.stream()).map_err(|error| {
            WorkspaceTestError::new(format!(
                "Rust source {} has a non-literal {} path: {error}",
                source.display(),
                kind.name(),
            ))
        })?;
        references.push(FileMacroReference {
            kind,
            path: argument.path.value(),
        });
        index += 3;
    }
    Ok(references)
}

fn item_attributes(item: &Item) -> &[syn::Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

fn public_visibility(visibility: &syn::Visibility) -> bool {
    matches!(visibility, syn::Visibility::Public(_))
}

fn public_reexports(items: &[Item], module_depth: usize) -> (Vec<PublicUse>, bool) {
    let mut reexports = Vec::new();
    let mut uncertain = false;
    for item in items {
        let Item::Use(item) = item else {
            continue;
        };
        if !public_visibility(&item.vis) || exact_cfg_test(&item.attrs) {
            continue;
        }
        collect_public_uses(
            &item.tree,
            module_depth,
            Vec::new(),
            &mut reexports,
            &mut uncertain,
        );
    }
    (reexports, uncertain)
}

fn collect_public_uses(
    tree: &syn::UseTree,
    module_depth: usize,
    prefix: Vec<String>,
    reexports: &mut Vec<PublicUse>,
    uncertain: &mut bool,
) {
    match tree {
        syn::UseTree::Path(path) if path.ident == "self" => {
            collect_public_uses(&path.tree, module_depth, prefix, reexports, uncertain);
        }
        syn::UseTree::Path(path) if path.ident == "crate" && module_depth == 0 => {
            collect_public_uses(&path.tree, module_depth, prefix, reexports, uncertain);
        }
        syn::UseTree::Path(path) if path.ident == "crate" || path.ident == "super" => {
            *uncertain = true;
        }
        syn::UseTree::Path(path) => {
            let mut prefix = prefix;
            prefix.push(path.ident.to_string());
            collect_public_uses(&path.tree, module_depth, prefix, reexports, uncertain);
        }
        syn::UseTree::Name(name) => {
            let mut path = prefix;
            path.push(name.ident.to_string());
            reexports.push(PublicUse {
                path,
                exported_name: name.ident.to_string(),
            });
        }
        syn::UseTree::Rename(rename) => {
            let mut path = prefix;
            path.push(rename.ident.to_string());
            reexports.push(PublicUse {
                path,
                exported_name: rename.rename.to_string(),
            });
        }
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                collect_public_uses(tree, module_depth, prefix.clone(), reexports, uncertain);
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
    reach: DeclaredReach,
) {
    // One production edge is enough to make a multiply-declared source part
    // of ordinary library reach; test-only survives only when every edge is.
    declared
        .entry(source)
        .and_modify(|existing| {
            existing.test_only &= reach.test_only;
            existing.dependent_tests |= reach.dependent_tests;
            existing.public_namespace |= reach.public_namespace;
        })
        .or_insert(reach);
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
