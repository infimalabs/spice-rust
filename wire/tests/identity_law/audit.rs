// Source audit proving that every public export has structural or sampled coverage.

/// Fail by name when a re-exported type has no executable or structural proof.
fn assert_coverage_is_complete(covered: &BTreeSet<String>) {
    let source = fs::read_to_string(lib_source()).expect("wire/src/lib.rs is readable");
    let exports = parse_exports(&source).expect("wire/src/lib.rs re-exports parse");
    let modules = read_module_sources(&exports);
    let audit = audit_coverage(&source, &modules, covered).expect("wire source declarations parse");

    assert!(
        audit.unaccounted.is_empty(),
        "these wire re-exports obey no stated identity law: {:?}. Exercise a \
         custom crossing type in every_crossing_type_obeys_the_identity_law. \
         Mark a process-only type beside its declaration with `identity-law: \
         non-crossing - <reason>`.",
        audit.unaccounted,
    );
    assert!(
        audit.stale.is_empty(),
        "these names have law samples but are no longer re-exported: {:?}",
        audit.stale,
    );
}

#[derive(Debug)]
struct Export {
    module: String,
    source_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclarationClass {
    Structural,
    Error,
    Trait,
    NonCrossing,
    Custom,
}

#[derive(Debug, PartialEq, Eq)]
struct CoverageAudit {
    unaccounted: BTreeMap<String, String>,
    stale: BTreeSet<String>,
}

/// Parse root re-exports and classify their declarations where those declarations live.
fn audit_coverage(
    root_source: &str,
    module_sources: &BTreeMap<String, String>,
    covered: &BTreeSet<String>,
) -> Result<CoverageAudit, String> {
    let exports = parse_exports(root_source)?;
    let mut declarations = BTreeMap::new();

    for (module, source) in module_sources {
        let file = syn::parse_file(source)
            .map_err(|error| format!("could not parse module {module}: {error}"))?;
        for item in &file.items {
            if let Some((name, class)) = classify_declaration(item)? {
                declarations.insert((module.clone(), name), class);
            }
        }
    }

    let mut unaccounted = BTreeMap::new();
    for (exported_name, export) in &exports {
        if covered.contains(exported_name) {
            continue;
        }

        let key = (export.module.clone(), export.source_name.clone());
        match declarations.get(&key) {
            Some(
                DeclarationClass::Structural
                | DeclarationClass::Error
                | DeclarationClass::Trait
                | DeclarationClass::NonCrossing,
            ) => {}
            Some(DeclarationClass::Custom) => {
                unaccounted.insert(
                    exported_name.clone(),
                    format!(
                        "{}::{} has custom or potentially narrowing encoding",
                        export.module, export.source_name
                    ),
                );
            }
            None => {
                unaccounted.insert(
                    exported_name.clone(),
                    format!(
                        "{}::{} has no source declaration the audit can classify",
                        export.module, export.source_name
                    ),
                );
            }
        }
    }

    let exported: BTreeSet<String> = exports.into_keys().collect();
    Ok(CoverageAudit {
        unaccounted,
        stale: covered.difference(&exported).cloned().collect(),
    })
}

fn parse_exports(source: &str) -> Result<BTreeMap<String, Export>, String> {
    let file = syn::parse_file(source).map_err(|error| error.to_string())?;
    let mut exports = BTreeMap::new();
    for item in file.items {
        let Item::Use(item_use) = item else {
            continue;
        };
        if !matches!(item_use.vis, Visibility::Public(_)) {
            continue;
        }
        collect_exports(&item_use.tree, &mut Vec::new(), &mut exports)?;
    }
    if exports.is_empty() {
        return Err("no public re-exports found".to_owned());
    }
    Ok(exports)
}

fn collect_exports(
    tree: &UseTree,
    path: &mut Vec<String>,
    exports: &mut BTreeMap<String, Export>,
) -> Result<(), String> {
    match tree {
        UseTree::Path(item) => {
            path.push(item.ident.to_string());
            collect_exports(&item.tree, path, exports)?;
            path.pop();
        }
        UseTree::Name(item) => {
            insert_export(
                path,
                item.ident.to_string(),
                item.ident.to_string(),
                exports,
            )?;
        }
        UseTree::Rename(item) => {
            insert_export(
                path,
                item.ident.to_string(),
                item.rename.to_string(),
                exports,
            )?;
        }
        UseTree::Group(item) => {
            for child in &item.items {
                collect_exports(child, path, exports)?;
            }
        }
        UseTree::Glob(_) => {
            return Err("glob re-exports cannot prove which wire types crossed".to_owned());
        }
    }
    Ok(())
}

fn insert_export(
    path: &[String],
    source_name: String,
    exported_name: String,
    exports: &mut BTreeMap<String, Export>,
) -> Result<(), String> {
    if path.is_empty() {
        return Err(format!(
            "root re-export {exported_name} has no owning module"
        ));
    }
    let export = Export {
        module: path.join("::"),
        source_name,
    };
    if exports.insert(exported_name.clone(), export).is_some() {
        return Err(format!("root name {exported_name} is re-exported twice"));
    }
    Ok(())
}

fn read_module_sources(exports: &BTreeMap<String, Export>) -> BTreeMap<String, String> {
    let source_dir = lib_source()
        .parent()
        .expect("wire/src/lib.rs has a parent")
        .to_path_buf();
    let modules: BTreeSet<&str> = exports
        .values()
        .map(|export| export.module.as_str())
        .collect();

    modules
        .into_iter()
        .map(|module| {
            let path = source_dir
                .join(module.replace("::", "/"))
                .with_extension("rs");
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
            (module.to_owned(), source)
        })
        .collect()
}

fn classify_declaration(item: &Item) -> Result<Option<(String, DeclarationClass)>, String> {
    match item {
        Item::Struct(item) if matches!(item.vis, Visibility::Public(_)) => Ok(Some((
            item.ident.to_string(),
            classify_data(&item.attrs, fields_attributes(&item.fields))?,
        ))),
        Item::Enum(item) if matches!(item.vis, Visibility::Public(_)) => {
            let nested = item.variants.iter().flat_map(|variant| {
                variant
                    .attrs
                    .iter()
                    .chain(fields_attributes(&variant.fields))
            });
            Ok(Some((
                item.ident.to_string(),
                classify_data(&item.attrs, nested)?,
            )))
        }
        Item::Trait(item) if matches!(item.vis, Visibility::Public(_)) => {
            Ok(Some((item.ident.to_string(), DeclarationClass::Trait)))
        }
        Item::Type(item) if matches!(item.vis, Visibility::Public(_)) => {
            Ok(Some((item.ident.to_string(), DeclarationClass::Custom)))
        }
        _ => Ok(None),
    }
}

fn fields_attributes(fields: &Fields) -> impl Iterator<Item = &Attribute> {
    fields.iter().flat_map(|field| field.attrs.iter())
}

fn classify_data<'a>(
    attrs: &[Attribute],
    nested_attrs: impl Iterator<Item = &'a Attribute>,
) -> Result<DeclarationClass, String> {
    if has_non_crossing_marker(attrs)? {
        return Ok(DeclarationClass::NonCrossing);
    }

    let derives = derive_names(attrs)?;
    if derives.contains("Error") {
        return Ok(DeclarationClass::Error);
    }

    let nested_attrs: Vec<&Attribute> = nested_attrs.collect();
    let structural = derives.contains("Serialize")
        && derives.contains("Deserialize")
        && derives.contains("PartialEq")
        && !has_narrowing_serde(attrs.iter().chain(nested_attrs))?;
    Ok(if structural {
        DeclarationClass::Structural
    } else {
        DeclarationClass::Custom
    })
}

fn derive_names(attrs: &[Attribute]) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("derive")) {
        let paths = attr
            .parse_args_with(syn::punctuated::Punctuated::<syn::Path, Token![,]>::parse_terminated)
            .map_err(|error| format!("could not parse derive attribute: {error}"))?;
        for path in paths {
            let name = path
                .segments
                .last()
                .expect("a derive path has one segment")
                .ident
                .to_string();
            names.insert(name);
        }
    }
    Ok(names)
}

fn has_narrowing_serde<'a>(attrs: impl Iterator<Item = &'a Attribute>) -> Result<bool, String> {
    const NARROWING: &[&str] = &[
        "flatten",
        "from",
        "into",
        "skip",
        "skip_deserializing",
        "skip_serializing",
        "skip_serializing_if",
        "try_from",
        "with",
        "deserialize_with",
        "serialize_with",
    ];

    for attr in attrs.filter(|attr| attr.path().is_ident("serde")) {
        let metas = attr
            .parse_args_with(syn::punctuated::Punctuated::<Meta, Token![,]>::parse_terminated)
            .map_err(|error| format!("could not parse serde attribute: {error}"))?;
        for meta in metas {
            let Some(name) = meta.path().segments.last() else {
                continue;
            };
            if NARROWING.contains(&name.ident.to_string().as_str()) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn has_non_crossing_marker(attrs: &[Attribute]) -> Result<bool, String> {
    let mut found = false;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("doc")) {
        let Meta::NameValue(meta) = &attr.meta else {
            continue;
        };
        let syn::Expr::Lit(expression) = &meta.value else {
            continue;
        };
        let syn::Lit::Str(line) = &expression.lit else {
            continue;
        };
        let line = line.value();
        let line = line.trim();
        if !line.contains("identity-law:") {
            continue;
        }
        let reason = line
            .strip_prefix("identity-law: non-crossing -")
            .ok_or_else(|| format!("malformed identity-law source marker: {line:?}"))?
            .trim();
        if reason.is_empty() {
            return Err("identity-law non-crossing marker has no reason".to_owned());
        }
        if found {
            return Err("declaration has more than one identity-law source marker".to_owned());
        }
        found = true;
    }
    Ok(found)
}

fn lib_source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("lib.rs")
}

#[test]
fn a_concurrent_structural_export_needs_no_shared_registry_edit() {
    let root = "pub use landed::{ConcurrentDto, ParseError, ProcessOnly};";
    let module = r#"
        #[derive(Serialize, Deserialize, PartialEq)]
        #[serde(deny_unknown_fields)]
        pub struct ConcurrentDto { pub value: String }

        #[derive(Debug, thiserror::Error)]
        #[error("bad input")]
        pub struct ParseError;

        /// identity-law: non-crossing - process-local registry metadata.
        pub struct ProcessOnly;
    "#;
    let modules = BTreeMap::from([("landed".to_owned(), module.to_owned())]);
    let audit = audit_coverage(root, &modules, &BTreeSet::new()).expect("fixture parses");

    assert_eq!(
        audit,
        CoverageAudit {
            unaccounted: BTreeMap::new(),
            stale: BTreeSet::new(),
        }
    );
}

#[test]
fn a_custom_encoder_still_requires_executable_law_samples() {
    let root = "pub use landed::CustomCodec;";
    let module = r#"
        #[derive(Serialize, Deserialize, PartialEq)]
        pub struct CustomCodec {
            #[serde(skip)]
            hidden: String,
        }
    "#;
    let modules = BTreeMap::from([("landed".to_owned(), module.to_owned())]);
    let uncovered = audit_coverage(root, &modules, &BTreeSet::new()).expect("fixture parses");
    assert_eq!(
        uncovered.unaccounted.keys().collect::<Vec<_>>(),
        vec!["CustomCodec"]
    );

    let covered = BTreeSet::from(["CustomCodec".to_owned()]);
    let sampled = audit_coverage(root, &modules, &covered).expect("fixture parses");
    assert!(sampled.unaccounted.is_empty());
    assert!(sampled.stale.is_empty());
}
