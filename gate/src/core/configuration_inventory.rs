//! Configuration declaration and consumer inventories from one selected tree.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use syn::parse::{Parse, ParseStream};
use syn::visit::{self, Visit};
use syn::{Expr, FnArg, Ident, ItemFn, ItemMacro, Lit, LitStr, Pat, Token, Type, braced};

use super::acquire::CoreSource;
use super::{ConfigInventoryEvidence, CoreEvidence, CoreInputError, CoreObservation};

const CONFIG_REGISTRY_PATH: &str = "spice/src/config.rs";

/// Parsed production Rust sources shared by configuration inventory providers.
pub(super) struct ConfigurationInventorySource {
    files: Vec<ParsedSource>,
}

struct ParsedSource {
    path: String,
    syntax: syn::File,
}

impl ConfigurationInventorySource {
    /// Parse every admitted production Rust source from the selected tree once.
    pub(super) fn capture(source: &CoreSource<'_>) -> Result<Self, CoreInputError> {
        let mut files = Vec::new();
        for entry in source
            .jurisdiction()
            .entries()
            .iter()
            .filter(|entry| entry.measurable() && production_rust(entry.path()))
        {
            let path = entry.path().to_owned();
            let text = fs::read_to_string(source.root().join(&path)).map_err(|error| {
                CoreInputError::new(format!(
                    "configuration inventory cannot read {path}: {error}"
                ))
            })?;
            let syntax = syn::parse_file(&text).map_err(|error| {
                CoreInputError::new(format!(
                    "configuration inventory cannot parse {path}: {error}"
                ))
            })?;
            files.push(ParsedSource { path, syntax });
        }
        Ok(Self { files })
    }

    /// Gather false-default registry declarations and their typed consumers.
    pub(super) fn false_disable(&self) -> Result<CoreObservation, CoreInputError> {
        let mut declared = BTreeSet::new();
        for source in self
            .files
            .iter()
            .filter(|source| source.path == CONFIG_REGISTRY_PATH)
        {
            let mut visitor = DeclarationVisitor::default();
            visitor.visit_file(&source.syntax);
            if let Some(error) = visitor.error {
                return Err(CoreInputError::new(format!(
                    "configuration inventory cannot inspect {} registry: {error}",
                    source.path
                )));
            }
            declared.extend(visitor.declared);
        }

        let mut consumer = ConsumerVisitor::new(&declared);
        for source in &self.files {
            consumer.visit_file(&source.syntax);
        }
        let observed = std::mem::take(&mut consumer.observed);
        drop(consumer);
        let evidence = ConfigInventoryEvidence::new(
            declared.into_iter().collect(),
            observed.into_iter().collect(),
            Vec::new(),
        )?;
        Ok(CoreObservation::Measured {
            evidence: CoreEvidence::ConfigFalseDisable(evidence),
        })
    }
}

fn production_rust(path: &str) -> bool {
    path.starts_with("spice/src/")
        && path.ends_with(".rs")
        && !path.ends_with("/tests.rs")
        && !path.contains("/tests/")
}

#[derive(Default)]
struct DeclarationVisitor {
    declared: Vec<Vec<String>>,
    error: Option<syn::Error>,
}

impl<'ast> Visit<'ast> for DeclarationVisitor {
    fn visit_item_macro(&mut self, node: &'ast ItemMacro) {
        if self.error.is_none()
            && node
                .mac
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "registry")
        {
            match syn::parse2::<RegistryRows>(node.mac.tokens.clone()) {
                Ok(rows) => {
                    self.declared.extend(
                        rows.0
                            .into_iter()
                            .filter(|row| row.false_builtin)
                            .map(|row| row.path),
                    );
                }
                Err(error) => self.error = Some(error),
            }
        }
        visit::visit_item_macro(self, node);
    }
}

struct RegistryRows(Vec<RegistryRow>);

struct RegistryRow {
    path: Vec<String>,
    false_builtin: bool,
}

impl Parse for RegistryRows {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut rows = Vec::new();
        while !input.is_empty() {
            let _: Ident = input.parse()?;
            let path: LitStr = input.parse()?;
            let body;
            braced!(body in input);
            let mut false_builtin = false;
            while !body.is_empty() {
                let field: Ident = body.parse()?;
                body.parse::<Token![:]>()?;
                let value: Expr = body.parse()?;
                body.parse::<Token![,]>()?;
                if field == "builtin" {
                    false_builtin = is_false_builtin(&value);
                }
            }
            rows.push(RegistryRow {
                path: path.value().split('.').map(str::to_owned).collect(),
                false_builtin,
            });
        }
        Ok(Self(rows))
    }
}

fn is_false_builtin(expression: &Expr) -> bool {
    let Some(boolean) = sole_call_argument(expression, "Some") else {
        return false;
    };
    let Some(value) = sole_call_argument(boolean, "Boolean") else {
        return false;
    };
    matches!(
        ungroup(value),
        Expr::Lit(literal) if matches!(&literal.lit, Lit::Bool(value) if !value.value)
    )
}

fn sole_call_argument<'a>(expression: &'a Expr, name: &str) -> Option<&'a Expr> {
    let Expr::Call(call) = ungroup(expression) else {
        return None;
    };
    let Expr::Path(function) = ungroup(&call.func) else {
        return None;
    };
    if call.args.len() != 1
        || !function
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == name)
    {
        return None;
    }
    call.args.first()
}

fn ungroup(expression: &Expr) -> &Expr {
    match expression {
        Expr::Group(group) => ungroup(&group.expr),
        Expr::Paren(parenthesized) => ungroup(&parenthesized.expr),
        _ => expression,
    }
}

struct ConsumerVisitor<'a> {
    declared: &'a BTreeSet<Vec<String>>,
    observed: BTreeSet<Vec<String>>,
    aliases: BTreeMap<String, Vec<String>>,
}

impl<'a> ConsumerVisitor<'a> {
    fn new(declared: &'a BTreeSet<Vec<String>>) -> Self {
        Self {
            declared,
            observed: BTreeSet::new(),
            aliases: BTreeMap::new(),
        }
    }
}

impl<'ast> Visit<'ast> for ConsumerVisitor<'_> {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let outer = self.aliases.clone();
        for argument in &node.sig.inputs {
            let FnArg::Typed(argument) = argument else {
                continue;
            };
            if root_type(&argument.ty)
                && let Pat::Ident(binding) = &*argument.pat
            {
                self.aliases.insert(binding.ident.to_string(), Vec::new());
            }
        }
        visit::visit_item_fn(self, node);
        self.aliases = outer;
    }

    fn visit_block(&mut self, node: &'ast syn::Block) {
        let outer = self.aliases.clone();
        visit::visit_block(self, node);
        self.aliases = outer;
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let Pat::Ident(binding) = &node.pat {
            let name = binding.ident.to_string();
            let path = node
                .init
                .as_ref()
                .and_then(|initial| method_path(&initial.expr, &self.aliases));
            if let Some(path) = path {
                self.aliases.insert(name, path);
            } else {
                self.aliases.remove(&name);
            }
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if let Some(path) = method_call_path(node, &self.aliases)
            && self.declared.contains(&path)
        {
            self.observed.insert(path);
        }
        visit::visit_expr_method_call(self, node);
    }
}

fn method_path(expression: &Expr, aliases: &BTreeMap<String, Vec<String>>) -> Option<Vec<String>> {
    match ungroup(expression) {
        Expr::MethodCall(call) => method_call_path(call, aliases),
        Expr::Path(path) if path.path.segments.len() == 1 => {
            let name = path.path.segments.first()?.ident.to_string();
            aliases.get(&name).cloned()
        }
        Expr::Reference(reference) => method_path(&reference.expr, aliases),
        _ => None,
    }
}

fn method_call_path(
    call: &syn::ExprMethodCall,
    aliases: &BTreeMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    let mut path = method_path(&call.receiver, aliases)?;
    path.push(call.method.to_string());
    Some(path)
}

fn root_type(kind: &Type) -> bool {
    match kind {
        Type::Group(group) => root_type(&group.elem),
        Type::Paren(parenthesized) => root_type(&parenthesized.elem),
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Root"),
        Type::Reference(reference) => root_type(&reference.elem),
        _ => false,
    }
}
