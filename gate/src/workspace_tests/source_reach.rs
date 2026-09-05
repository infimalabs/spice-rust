use super::*;
use proc_macro2::{TokenStream, TokenTree};
use syn::parse::Parser;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::visit::Visit;

mod macro_scope;

use macro_scope::{
    FileMacroBindings, FileMacroResolution, UseBindingContext, attribute_may_expand_items,
    conditional_binding, exported_macro_rules, generated_namespace_is_unresolved,
    local_macro_rules, macro_rules_name,
};

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
    scope: FileMacroScope,
}

struct PublicUse {
    path: Vec<String>,
    exported_name: String,
}

struct SourceGraph {
    selected_root: PathBuf,
    namespace_context: FileMacroCrateContext,
    declared: BTreeMap<PathBuf, DeclaredReach>,
    visited: BTreeSet<SourceVisit>,
    exported_macros: BTreeSet<String>,
    conservative_downstream: bool,
}

#[derive(Clone)]
struct SourceWalk {
    module_dir: PathBuf,
    reach: DeclaredReach,
    inherited_reexports: Vec<Vec<String>>,
    module_depth: usize,
    scope: Option<FileMacroScope>,
    crate_root: bool,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum FileMacroKind {
    Include,
    IncludeStr,
    IncludeBytes,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum NamespaceResolution {
    Compiler,
    Shadowed,
    Unresolved,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum RustEdition {
    Edition2015,
    Modern,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct FileMacroCrateContext {
    edition: RustEdition,
    std_extern: NamespaceResolution,
    core_extern: NamespaceResolution,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct FileMacroScope {
    edition: RustEdition,
    crate_std: NamespaceResolution,
    crate_core: NamespaceResolution,
    local_std: NamespaceResolution,
    local_core: NamespaceResolution,
    global_std: NamespaceResolution,
    global_core: NamespaceResolution,
    macros: FileMacroBindings,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum QualifiedNamespace {
    Std,
    Core,
}

impl QualifiedNamespace {
    fn name(self) -> &'static str {
        match self {
            Self::Std => "std",
            Self::Core => "core",
        }
    }
}

#[derive(Clone, Copy)]
enum FileMacroPath {
    Unqualified(FileMacroKind),
    Qualified {
        namespace: QualifiedNamespace,
        kind: FileMacroKind,
        leading_global: bool,
    },
}

impl FileMacroCrateContext {
    pub(super) fn from_metadata(
        package: &str,
        edition: &str,
        dependencies: &[MetadataDependency],
    ) -> Result<Self, WorkspaceTestError> {
        let edition = match edition {
            "2015" => RustEdition::Edition2015,
            "2018" | "2021" | "2024" => RustEdition::Modern,
            edition => {
                return Err(WorkspaceTestError::new(format!(
                    "workspace package {package:?} has unsupported Rust edition {edition:?}",
                )));
            }
        };
        let mut std_extern = NamespaceResolution::Compiler;
        let mut core_extern = NamespaceResolution::Compiler;
        for dependency in dependencies {
            let name = dependency.rename.as_ref().unwrap_or(&dependency.name);
            match name.replace('-', "_").as_str() {
                "std" => std_extern = NamespaceResolution::Unresolved,
                "core" => core_extern = NamespaceResolution::Unresolved,
                _ => {}
            }
        }
        Ok(Self {
            edition,
            std_extern,
            core_extern,
        })
    }

    #[cfg(test)]
    pub(super) fn test_default() -> Self {
        Self {
            edition: RustEdition::Modern,
            std_extern: NamespaceResolution::Compiler,
            core_extern: NamespaceResolution::Compiler,
        }
    }

    fn root_scope(self, attributes: &[syn::Attribute]) -> FileMacroScope {
        let no_std = attributes
            .iter()
            .any(|attribute| attribute.path().is_ident("no_std"));
        let no_implicit_prelude = attributes
            .iter()
            .any(|attribute| attribute.path().is_ident("no_implicit_prelude"));
        let std_extern = if no_std || no_implicit_prelude {
            NamespaceResolution::Unresolved
        } else {
            self.std_extern
        };
        let core_extern = if no_implicit_prelude {
            NamespaceResolution::Unresolved
        } else {
            self.core_extern
        };
        FileMacroScope {
            edition: self.edition,
            crate_std: self.std_extern,
            crate_core: self.core_extern,
            local_std: std_extern,
            local_core: core_extern,
            global_std: std_extern,
            global_core: core_extern,
            macros: FileMacroBindings::compiler(),
        }
    }
}

impl FileMacroScope {
    fn resolve(self, namespace: QualifiedNamespace, leading_global: bool) -> NamespaceResolution {
        match (namespace, leading_global) {
            (QualifiedNamespace::Std, false) => self.local_std,
            (QualifiedNamespace::Core, false) => self.local_core,
            (QualifiedNamespace::Std, true) => self.global_std,
            (QualifiedNamespace::Core, true) => self.global_core,
        }
    }

    fn child_module(self) -> Self {
        Self {
            local_std: self.global_std,
            local_core: self.global_core,
            macros: self.macros.child_module(),
            ..self
        }
    }

    fn child_block(self) -> Self {
        Self {
            macros: self.macros.child_block(),
            ..self
        }
    }

    fn resolve_macro(self, name: FileMacroKind) -> FileMacroResolution {
        self.macros.resolve(name)
    }

    fn with_items(
        mut self,
        items: &[Item],
        crate_root: bool,
        exported_macros: &BTreeSet<String>,
    ) -> Self {
        let local_macros = local_macro_rules(items);
        for item in items {
            self.apply_item(item, crate_root, exported_macros, &local_macros);
        }
        self
    }

    fn with_block_items(mut self, block: &syn::Block, exported_macros: &BTreeSet<String>) -> Self {
        let local_macros = block
            .stmts
            .iter()
            .filter_map(|statement| match statement {
                syn::Stmt::Item(item) if !conditional_binding(item_attributes(item)) => {
                    macro_rules_name(item)
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        for statement in &block.stmts {
            if let syn::Stmt::Item(item) = statement {
                self.apply_item(item, false, exported_macros, &local_macros);
            }
        }
        for statement in &block.stmts {
            if let syn::Stmt::Item(item) = statement
                && generated_namespace_is_unresolved(item)
            {
                self.mark_unresolved(QualifiedNamespace::Std, false);
                self.mark_unresolved(QualifiedNamespace::Core, false);
                self.macros.mark_imports_unresolved();
            } else if let syn::Stmt::Item(item) = statement
                && item_attributes(item).iter().any(attribute_may_expand_items)
            {
                self.macros.mark_imports_unresolved();
            }
        }
        self
    }

    fn with_generated_items(mut self, items: &[Item], crate_root: bool) -> Self {
        if items.iter().any(generated_namespace_is_unresolved) {
            self.mark_unresolved(QualifiedNamespace::Std, crate_root);
            self.mark_unresolved(QualifiedNamespace::Core, crate_root);
            self.macros.mark_imports_unresolved();
        }
        if items
            .iter()
            .flat_map(item_attributes)
            .any(attribute_may_expand_items)
        {
            self.macros.mark_imports_unresolved();
        }
        self
    }

    fn with_textual_item(mut self, item: &Item) -> Self {
        if let Some(name) = macro_rules_name(item)
            && let Some(kind) = FileMacroKind::from_name(&name)
        {
            let resolution = if conditional_binding(item_attributes(item)) {
                FileMacroResolution::Unresolved
            } else {
                FileMacroResolution::Shadowed
            };
            self.macros.bind_textual(kind, resolution);
        }
        self
    }

    fn apply_item(
        &mut self,
        item: &Item,
        crate_root: bool,
        exported_macros: &BTreeSet<String>,
        local_macros: &BTreeSet<String>,
    ) {
        if item_attributes(item)
            .iter()
            .any(|attribute| attribute.path().is_ident("macro_use"))
        {
            self.macros.mark_imports_unresolved();
        }
        match item {
            Item::ExternCrate(item) => {
                let bound = item
                    .rename
                    .as_ref()
                    .map_or_else(|| ident_name(&item.ident), |(_, name)| ident_name(name));
                let source = ident_name(&item.ident);
                if let Some(namespace) = qualified_namespace(&bound) {
                    let resolution = if let Some(source_namespace) = qualified_namespace(&source) {
                        self.crate_resolution(source_namespace)
                    } else {
                        NamespaceResolution::Shadowed
                    };
                    self.bind(namespace, resolution, crate_root);
                }
            }
            Item::Mod(item) => {
                if let Some(namespace) = qualified_namespace(&ident_name(&item.ident)) {
                    self.bind(namespace, NamespaceResolution::Shadowed, false);
                    if crate_root && matches!(self.edition, RustEdition::Edition2015) {
                        self.bind_global(namespace, NamespaceResolution::Shadowed);
                    }
                }
            }
            Item::Use(item) => {
                self.apply_use_tree(
                    &item.tree,
                    &mut Vec::new(),
                    UseBindingContext {
                        crate_root,
                        leading_global: item.leading_colon.is_some(),
                        exported_macros,
                        local_macros,
                        conditional: conditional_binding(&item.attrs),
                    },
                );
            }
            _ => {}
        }
    }

    fn apply_use_tree(
        &mut self,
        tree: &syn::UseTree,
        prefix: &mut Vec<String>,
        context: UseBindingContext<'_>,
    ) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(ident_name(&path.ident));
                self.apply_use_tree(&path.tree, prefix, context);
                prefix.pop();
            }
            syn::UseTree::Name(name) => {
                let name = ident_name(&name.ident);
                let (source, bound) = if name == "self" {
                    (prefix.clone(), prefix.last().cloned())
                } else {
                    let mut source = prefix.clone();
                    source.push(name.clone());
                    (source, Some(name))
                };
                if let Some(bound) = bound {
                    self.bind_use(&source, &bound, context);
                }
            }
            syn::UseTree::Rename(rename) => {
                let mut source = prefix.clone();
                let name = ident_name(&rename.ident);
                if name != "self" {
                    source.push(name);
                }
                self.bind_use(&source, &ident_name(&rename.rename), context);
            }
            syn::UseTree::Glob(_) => {
                self.mark_unresolved(QualifiedNamespace::Std, context.crate_root);
                self.mark_unresolved(QualifiedNamespace::Core, context.crate_root);
                self.macros.mark_imports_unresolved();
            }
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.apply_use_tree(tree, prefix, context);
                }
            }
        }
    }

    fn bind_use(&mut self, source: &[String], bound: &str, context: UseBindingContext<'_>) {
        if let Some(namespace) = qualified_namespace(bound) {
            let resolution = if let [source] = source
                && let Some(source_namespace) = qualified_namespace(source)
            {
                self.resolve(source_namespace, context.leading_global)
            } else {
                NamespaceResolution::Shadowed
            };
            self.bind(namespace, resolution, false);
            if context.crate_root && matches!(self.edition, RustEdition::Edition2015) {
                self.bind_global(namespace, resolution);
            }
        }
        if let Some(name) = FileMacroKind::from_name(bound) {
            let resolution = if context.conditional {
                FileMacroResolution::Unresolved
            } else {
                self.imported_macro_resolution(
                    source,
                    context.leading_global,
                    context.exported_macros,
                    context.local_macros,
                )
            };
            self.macros.bind_import(name, resolution);
        }
    }

    fn imported_macro_resolution(
        self,
        source: &[String],
        leading_global: bool,
        exported_macros: &BTreeSet<String>,
        local_macros: &BTreeSet<String>,
    ) -> FileMacroResolution {
        if let [namespace, name] = source
            && let Some(namespace) = qualified_namespace(namespace)
            && let Some(kind) = FileMacroKind::from_name(name)
        {
            return match self.resolve(namespace, leading_global) {
                NamespaceResolution::Compiler => FileMacroResolution::Compiler(kind),
                NamespaceResolution::Shadowed | NamespaceResolution::Unresolved => {
                    FileMacroResolution::Unresolved
                }
            };
        }
        let known_user_macro = match source {
            [root, name] if root == "crate" => exported_macros.contains(name),
            [root, name] if root == "self" => local_macros.contains(name),
            [name] => local_macros.contains(name),
            _ => false,
        };
        if known_user_macro {
            FileMacroResolution::Shadowed
        } else {
            FileMacroResolution::Unresolved
        }
    }

    fn bind(
        &mut self,
        namespace: QualifiedNamespace,
        resolution: NamespaceResolution,
        global: bool,
    ) {
        match namespace {
            QualifiedNamespace::Std => self.local_std = resolution,
            QualifiedNamespace::Core => self.local_core = resolution,
        }
        if global {
            self.bind_global(namespace, resolution);
        }
    }

    fn bind_global(&mut self, namespace: QualifiedNamespace, resolution: NamespaceResolution) {
        match namespace {
            QualifiedNamespace::Std => self.global_std = resolution,
            QualifiedNamespace::Core => self.global_core = resolution,
        }
    }

    fn mark_unresolved(&mut self, namespace: QualifiedNamespace, crate_root: bool) {
        if matches!(
            self.resolve(namespace, false),
            NamespaceResolution::Compiler
        ) {
            self.bind(namespace, NamespaceResolution::Unresolved, false);
        }
        if crate_root
            && matches!(self.edition, RustEdition::Edition2015)
            && matches!(self.resolve(namespace, true), NamespaceResolution::Compiler)
        {
            self.bind_global(namespace, NamespaceResolution::Unresolved);
        }
    }

    fn crate_resolution(self, namespace: QualifiedNamespace) -> NamespaceResolution {
        match namespace {
            QualifiedNamespace::Std => self.crate_std,
            QualifiedNamespace::Core => self.crate_core,
        }
    }
}

fn qualified_namespace(name: &str) -> Option<QualifiedNamespace> {
    match name {
        "std" => Some(QualifiedNamespace::Std),
        "core" => Some(QualifiedNamespace::Core),
        _ => None,
    }
}

impl FileMacroKind {
    const ALL: [Self; 3] = [Self::Include, Self::IncludeStr, Self::IncludeBytes];

    const fn index(self) -> usize {
        match self {
            Self::Include => 0,
            Self::IncludeStr => 1,
            Self::IncludeBytes => 2,
        }
    }

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

    fn classify_path(path: &syn::Path) -> Option<FileMacroPath> {
        let segments = path.segments.iter().collect::<Vec<_>>();
        if segments
            .iter()
            .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
        {
            return None;
        }
        match segments.as_slice() {
            [name] if path.leading_colon.is_none() => {
                Self::from_name(&ident_name(&name.ident)).map(FileMacroPath::Unqualified)
            }
            [namespace, name]
                if matches!(ident_name(&namespace.ident).as_str(), "std" | "core") =>
            {
                let namespace = match ident_name(&namespace.ident).as_str() {
                    "std" => QualifiedNamespace::Std,
                    "core" => QualifiedNamespace::Core,
                    _ => unreachable!("qualified namespace spelling was checked"),
                };
                Self::from_name(&ident_name(&name.ident)).map(|kind| FileMacroPath::Qualified {
                    namespace,
                    kind,
                    leading_global: path.leading_colon.is_some(),
                })
            }
            _ => None,
        }
    }

    pub(super) fn from_path(path: &syn::Path) -> Option<Self> {
        match Self::classify_path(path)? {
            FileMacroPath::Unqualified(kind) | FileMacroPath::Qualified { kind, .. } => Some(kind),
        }
    }
}

struct FileMacroReference {
    kind: FileMacroKind,
    path: String,
    scope: FileMacroScope,
    crate_root: bool,
}

struct FileMacroArgument {
    path: Option<syn::LitStr>,
}

impl Parse for FileMacroArgument {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let expression = input.parse::<Expr>()?;
        let _trailing_comma = input.parse::<Option<syn::Token![,]>>()?;
        if !input.is_empty() {
            return Err(input.error("expected one file-macro path expression"));
        }
        match expression {
            Expr::Lit(expression) => match expression.lit {
                Lit::Str(path) => Ok(Self { path: Some(path) }),
                _ => Err(syn::Error::new_spanned(
                    expression,
                    "expected a string-literal path",
                )),
            },
            Expr::Macro(expression) if bounded_out_dir_concat(&expression.mac)? => {
                Ok(Self { path: None })
            }
            expression => Err(syn::Error::new_spanned(
                expression,
                "expected a string literal or concat!(env!(\"OUT_DIR\"), \"/literal\")",
            )),
        }
    }
}

fn bounded_out_dir_concat(invocation: &syn::Macro) -> syn::Result<bool> {
    if !invocation.path.is_ident("concat") {
        return Ok(false);
    }
    let parser = Punctuated::<Expr, syn::Token![,]>::parse_terminated;
    let arguments = parser.parse2(invocation.tokens.clone())?;
    let [Expr::Macro(environment), Expr::Lit(suffix)] =
        arguments.iter().collect::<Vec<_>>().as_slice()
    else {
        return Ok(false);
    };
    if !environment.mac.path.is_ident("env") {
        return Ok(false);
    }
    let variable = syn::parse2::<syn::LitStr>(environment.mac.tokens.clone())?;
    let Lit::Str(suffix) = &suffix.lit else {
        return Ok(false);
    };
    if variable.value() != "OUT_DIR" {
        return Ok(false);
    }
    let value = suffix.value();
    let Some(relative) = value.strip_prefix('/') else {
        return Ok(false);
    };
    let path = Path::new(relative);
    Ok(!relative.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_))))
}

struct FileMacroVisitor<'a> {
    source: &'a Path,
    exported_macros: &'a BTreeSet<String>,
    scope: FileMacroScope,
    crate_root: bool,
    references: Vec<FileMacroReference>,
    error: Option<WorkspaceTestError>,
}

/// Acquire exact external module and built-in file-macro sources with their Cargo target.
pub(super) fn declared_path_sources(
    selected_root: &Path,
    target_source: &Path,
    namespace_context: FileMacroCrateContext,
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
        namespace_context,
        declared: BTreeMap::new(),
        visited: BTreeSet::new(),
        exported_macros: BTreeSet::new(),
        conservative_downstream: false,
    };
    graph.collect_declared_sources(
        target_source,
        SourceWalk {
            module_dir: module_dir.to_path_buf(),
            reach: DeclaredReach {
                test_only: false,
                dependent_tests: true,
                public_namespace: true,
            },
            inherited_reexports: Vec::new(),
            module_depth: 0,
            scope: None,
            crate_root: true,
        },
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
        mut walk: SourceWalk,
    ) -> Result<(), WorkspaceTestError> {
        let source = fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
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
        if walk.crate_root {
            self.exported_macros
                .extend(exported_macro_rules(&syntax.items));
        }
        let scope = walk
            .scope
            .unwrap_or_else(|| self.namespace_context.root_scope(&syntax.attrs));
        if !self.visited.insert(SourceVisit {
            source: source.clone(),
            reach: walk.reach,
            inherited_reexports: walk.inherited_reexports.clone(),
            scope,
        }) {
            return Ok(());
        }
        walk.scope = Some(scope);
        self.collect_declared_items(&syntax.items, &source, walk)
    }

    fn collect_declared_items(
        &mut self,
        items: &[Item],
        source: &Path,
        walk: SourceWalk,
    ) -> Result<(), WorkspaceTestError> {
        let explicit_scope = walk
            .scope
            .expect("source walk scope is established before item traversal")
            .with_items(items, walk.crate_root, &self.exported_macros);
        let scope = explicit_scope.with_generated_items(items, walk.crate_root);
        let (local_reexports, uncertain_reexport) = public_reexports(items, walk.module_depth);
        if walk.reach.dependent_tests && uncertain_reexport {
            self.conservative_downstream = true;
        }
        let requested_names = walk
            .inherited_reexports
            .iter()
            .filter_map(|path| path.first())
            .collect::<BTreeSet<_>>();
        let mut reexport_paths = walk.inherited_reexports.clone();
        reexport_paths.extend(
            local_reexports
                .into_iter()
                .filter(|reexport| {
                    walk.reach.public_namespace
                        || (walk.reach.dependent_tests
                            && requested_names.contains(&reexport.exported_name))
                })
                .map(|reexport| reexport.path),
        );
        let mut textual_scope = scope;
        for item in items {
            let test_only = walk.reach.test_only || exact_cfg_test(item_attributes(item));
            let item_reach = DeclaredReach {
                test_only,
                dependent_tests: !test_only && walk.reach.dependent_tests,
                public_namespace: !test_only && walk.reach.public_namespace,
            };
            let item_scope = textual_scope.with_textual_item(item);
            let item_walk = SourceWalk {
                reach: item_reach,
                inherited_reexports: reexport_paths.clone(),
                scope: Some(item_scope),
                crate_root: walk.crate_root,
                ..walk.clone()
            };
            for reference in file_macro_references(
                item,
                source,
                item_scope,
                walk.crate_root,
                &self.exported_macros,
            )? {
                self.collect_file_macro_source(reference, source, &item_walk)?;
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
                let directly_public = walk.reach.public_namespace && public_visibility(&module.vis);
                let child_reach = DeclaredReach {
                    test_only: item_reach.test_only,
                    dependent_tests: !item_reach.test_only && (directly_public || reexported),
                    public_namespace: !item_reach.test_only
                        && (directly_public || reexported_namespace),
                };
                if let Some((_, inline)) = &module.content {
                    let child_dir = walk.module_dir.join(module_name);
                    self.collect_declared_items(
                        inline,
                        source,
                        SourceWalk {
                            module_dir: child_dir,
                            reach: child_reach,
                            inherited_reexports: child_reexports,
                            module_depth: walk.module_depth.saturating_add(1),
                            scope: Some(item_scope.child_module()),
                            crate_root: false,
                        },
                    )?;
                } else if let Some(module_source) =
                    external_module_source(module, source, &walk.module_dir)?
                {
                    let module_source = fs::canonicalize(&module_source).unwrap_or(module_source);
                    let child_dir = child_module_dir(&module_source)?;
                    record_declared_source(&mut self.declared, module_source.clone(), child_reach);
                    self.collect_declared_sources(
                        &module_source,
                        SourceWalk {
                            module_dir: child_dir,
                            reach: child_reach,
                            inherited_reexports: child_reexports,
                            module_depth: walk.module_depth.saturating_add(1),
                            scope: Some(item_scope.child_module()),
                            crate_root: false,
                        },
                    )?;
                }
            }
            textual_scope = item_scope;
        }
        Ok(())
    }

    fn collect_file_macro_source(
        &mut self,
        reference: FileMacroReference,
        source: &Path,
        walk: &SourceWalk,
    ) -> Result<(), WorkspaceTestError> {
        let included = self.admit_file_macro_source(source, &reference)?;
        record_declared_source(&mut self.declared, included.clone(), walk.reach);
        if matches!(reference.kind, FileMacroKind::Include) {
            self.collect_included_rust_source(
                &included,
                SourceWalk {
                    scope: Some(reference.scope),
                    crate_root: reference.crate_root,
                    ..walk.clone()
                },
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
        walk: SourceWalk,
    ) -> Result<(), WorkspaceTestError> {
        let scope = walk
            .scope
            .expect("included source retains its invocation scope");
        let visit = SourceVisit {
            source: source.to_path_buf(),
            reach: walk.reach,
            inherited_reexports: walk.inherited_reexports.clone(),
            scope,
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
            Ok(syntax) => self.collect_declared_items(&syntax.items, source, walk.clone()),
            Err(file_error) => {
                let tokens = document.parse::<TokenStream>().map_err(|token_error| {
                    WorkspaceTestError::new(format!(
                        "parse included Rust source {} while resolving staged test reach: {file_error}; tokenization also failed: {token_error}",
                        source.display()
                    ))
                })?;
                for reference in
                    file_macro_references_in_tokens(tokens, source, scope, walk.crate_root)?
                {
                    self.collect_file_macro_source(reference, source, &walk)?;
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
        match FileMacroKind::classify_path(&invocation.path) {
            Some(FileMacroPath::Unqualified(kind)) => {
                self.record_unqualified(kind, invocation.tokens.clone());
            }
            Some(FileMacroPath::Qualified {
                namespace,
                kind,
                leading_global,
            }) => self.record_qualified(namespace, kind, leading_global, invocation.tokens.clone()),
            None => self.scan(invocation.tokens.clone()),
        }
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        let outer = self.scope;
        let crate_root = self.crate_root;
        let mut scope = self
            .scope
            .child_block()
            .with_block_items(block, self.exported_macros);
        self.crate_root = false;
        for statement in &block.stmts {
            if let syn::Stmt::Item(item) = statement {
                scope = scope.with_textual_item(item);
            }
            self.scope = scope;
            syn::visit::visit_stmt(self, statement);
        }
        self.scope = outer;
        self.crate_root = crate_root;
    }

    fn visit_token_stream(&mut self, tokens: &'ast TokenStream) {
        self.scan(tokens.clone());
    }
}

impl FileMacroVisitor<'_> {
    fn record_unqualified(&mut self, name: FileMacroKind, tokens: TokenStream) {
        match self.scope.resolve_macro(name) {
            FileMacroResolution::Compiler(kind) => self.record(kind, tokens),
            FileMacroResolution::Shadowed => {}
            FileMacroResolution::Unresolved if self.error.is_none() => {
                self.error = Some(WorkspaceTestError::new(format!(
                    "Rust source {} cannot prove unqualified file-macro namespace for {}",
                    self.source.display(),
                    name.name(),
                )));
            }
            FileMacroResolution::Unresolved => {}
        }
    }

    fn record(&mut self, kind: FileMacroKind, tokens: TokenStream) {
        if self.error.is_some() {
            return;
        }
        match syn::parse2::<FileMacroArgument>(tokens) {
            Ok(FileMacroArgument { path: Some(path) }) => {
                self.references.push(FileMacroReference {
                    kind,
                    path: path.value(),
                    scope: self.scope,
                    crate_root: self.crate_root,
                });
            }
            Ok(FileMacroArgument { path: None }) => {}
            Err(error) => {
                self.error = Some(WorkspaceTestError::new(format!(
                    "Rust source {} has a non-literal {} path: {error}",
                    self.source.display(),
                    kind.name(),
                )));
            }
        }
    }

    fn record_qualified(
        &mut self,
        namespace: QualifiedNamespace,
        kind: FileMacroKind,
        leading_global: bool,
        tokens: TokenStream,
    ) {
        match self.scope.resolve(namespace, leading_global) {
            NamespaceResolution::Compiler => self.record(kind, tokens),
            NamespaceResolution::Shadowed => {}
            NamespaceResolution::Unresolved if self.error.is_none() => {
                let root = if leading_global { "::" } else { "" };
                self.error = Some(WorkspaceTestError::new(format!(
                    "Rust source {} cannot prove qualified file-macro namespace for {root}{}::{}",
                    self.source.display(),
                    namespace.name(),
                    kind.name(),
                )));
            }
            NamespaceResolution::Unresolved => {}
        }
    }

    fn scan(&mut self, tokens: TokenStream) {
        if self.error.is_some() {
            return;
        }
        match file_macro_references_in_tokens(tokens, self.source, self.scope, self.crate_root) {
            Ok(references) => self.references.extend(references),
            Err(error) => self.error = Some(error),
        }
    }
}

fn file_macro_references(
    item: &Item,
    source: &Path,
    scope: FileMacroScope,
    crate_root: bool,
    exported_macros: &BTreeSet<String>,
) -> Result<Vec<FileMacroReference>, WorkspaceTestError> {
    let mut visitor = FileMacroVisitor {
        source,
        exported_macros,
        scope,
        crate_root,
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
    scope: FileMacroScope,
    crate_root: bool,
) -> Result<Vec<FileMacroReference>, WorkspaceTestError> {
    let trees = tokens.into_iter().collect::<Vec<_>>();
    let mut references = Vec::new();
    let mut index = 0;
    while index < trees.len() {
        let Some((path, arguments_index, next_index)) = file_macro_invocation_at(&trees, index)
        else {
            if let TokenTree::Group(group) = &trees[index] {
                references.extend(file_macro_references_in_tokens(
                    group.stream(),
                    source,
                    scope,
                    crate_root,
                )?);
            }
            index += 1;
            continue;
        };
        let Some(TokenTree::Group(arguments)) = trees.get(arguments_index) else {
            unreachable!("macro invocation shape was checked")
        };
        let kind = match path {
            FileMacroPath::Unqualified(name) => match scope.resolve_macro(name) {
                FileMacroResolution::Compiler(kind) => kind,
                FileMacroResolution::Shadowed => {
                    index = next_index;
                    continue;
                }
                FileMacroResolution::Unresolved => {
                    return Err(WorkspaceTestError::new(format!(
                        "Rust source {} cannot prove unqualified file-macro namespace for {}",
                        source.display(),
                        name.name(),
                    )));
                }
            },
            FileMacroPath::Qualified {
                namespace,
                kind,
                leading_global,
            } => match scope.resolve(namespace, leading_global) {
                NamespaceResolution::Compiler => kind,
                NamespaceResolution::Shadowed => {
                    index = next_index;
                    continue;
                }
                NamespaceResolution::Unresolved => {
                    let root = if leading_global { "::" } else { "" };
                    return Err(WorkspaceTestError::new(format!(
                        "Rust source {} cannot prove qualified file-macro namespace for {root}{}::{}",
                        source.display(),
                        namespace.name(),
                        kind.name(),
                    )));
                }
            },
        };
        let argument = syn::parse2::<FileMacroArgument>(arguments.stream()).map_err(|error| {
            WorkspaceTestError::new(format!(
                "Rust source {} has a non-literal {} path: {error}",
                source.display(),
                kind.name(),
            ))
        })?;
        if let Some(path) = argument.path {
            references.push(FileMacroReference {
                kind,
                path: path.value(),
                scope,
                crate_root,
            });
        }
        index = next_index;
    }
    Ok(references)
}

fn file_macro_invocation_at(
    trees: &[TokenTree],
    index: usize,
) -> Option<(FileMacroPath, usize, usize)> {
    let TokenTree::Ident(first) = trees.get(index)? else {
        return None;
    };
    let unqualified = FileMacroKind::from_name(&ident_name(first))
        .filter(|_| !path_separator_before(trees, index))
        .map(|kind| (FileMacroPath::Unqualified(kind), index + 2, index + 3));
    if let Some((kind, arguments, next)) = unqualified
        && macro_arguments_at(trees, index + 1, arguments)
    {
        return Some((kind, arguments, next));
    }

    if !matches!(ident_name(first).as_str(), "std" | "core")
        || qualified_namespace_has_parent(trees, index)
        || !path_separator_at(trees, index + 1)
    {
        return None;
    }
    let TokenTree::Ident(name) = trees.get(index + 3)? else {
        return None;
    };
    let kind = FileMacroKind::from_name(&ident_name(name))?;
    let namespace = qualified_namespace(&ident_name(first))?;
    let path = FileMacroPath::Qualified {
        namespace,
        kind,
        leading_global: path_separator_before(trees, index),
    };
    macro_arguments_at(trees, index + 4, index + 5).then_some((path, index + 5, index + 6))
}

fn macro_arguments_at(trees: &[TokenTree], bang: usize, arguments: usize) -> bool {
    matches!(trees.get(bang), Some(TokenTree::Punct(punct)) if punct.as_char() == '!')
        && matches!(trees.get(arguments), Some(TokenTree::Group(_)))
}

fn path_separator_at(trees: &[TokenTree], index: usize) -> bool {
    matches!(trees.get(index), Some(TokenTree::Punct(punct)) if punct.as_char() == ':')
        && matches!(trees.get(index + 1), Some(TokenTree::Punct(punct)) if punct.as_char() == ':')
}

fn path_separator_before(trees: &[TokenTree], index: usize) -> bool {
    index >= 2 && path_separator_at(trees, index - 2)
}

fn qualified_namespace_has_parent(trees: &[TokenTree], index: usize) -> bool {
    path_separator_before(trees, index)
        && index >= 3
        && matches!(
            trees.get(index - 3),
            Some(TokenTree::Ident(_) | TokenTree::Group(_))
        )
}

fn ident_name(ident: &proc_macro2::Ident) -> String {
    let name = ident.to_string();
    name.strip_prefix("r#").unwrap_or(&name).to_owned()
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
