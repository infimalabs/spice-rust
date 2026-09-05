use super::super::source_reach::macro_scope::attribute_may_expand_items;
use super::super::source_reach::{ident_name, item_attributes};
use super::*;
use syn::visit::Visit;

#[derive(Clone, Copy)]
pub(super) struct OutputMacroAuthority {
    bare: OutputMacroResolution,
    standard: OutputMacroResolution,
}

#[derive(Clone, Copy)]
pub(super) enum OutputMacroResolution {
    Compiler,
    Unresolved,
}

impl OutputMacroAuthority {
    pub(super) fn compiler(compiler_std_extern: bool) -> Self {
        let initial = if compiler_std_extern {
            OutputMacroResolution::Compiler
        } else {
            OutputMacroResolution::Unresolved
        };
        Self {
            bare: initial,
            standard: initial,
        }
    }

    pub(super) fn from_file(file: &syn::File, compiler_std_extern: bool) -> Self {
        let implicit_standard = !file.attrs.iter().any(|attribute| {
            attribute.path().is_ident("no_std") || attribute.path().is_ident("no_implicit_prelude")
        });
        let initial = if compiler_std_extern && implicit_standard {
            OutputMacroResolution::Compiler
        } else {
            OutputMacroResolution::Unresolved
        };
        let mut collector = OutputMacroCollector {
            authority: Self {
                bare: initial,
                standard: initial,
            },
        };
        collector.visit_file(file);
        collector.authority
    }

    pub(super) fn merge(&mut self, other: Self) {
        if matches!(other.bare, OutputMacroResolution::Unresolved) {
            self.bare = OutputMacroResolution::Unresolved;
        }
        if matches!(other.standard, OutputMacroResolution::Unresolved) {
            self.standard = OutputMacroResolution::Unresolved;
        }
    }

    pub(super) fn resolve(self, path: &syn::Path) -> OutputMacroResolution {
        let segments = path
            .segments
            .iter()
            .map(|segment| ident_name(&segment.ident))
            .collect::<Vec<_>>();
        match segments.as_slice() {
            [name] if name == "println" && path.leading_colon.is_none() => self.bare,
            [root, name] if root == "std" && name == "println" => self.standard,
            _ => OutputMacroResolution::Unresolved,
        }
    }
}

struct OutputMacroCollector {
    authority: OutputMacroAuthority,
}

impl OutputMacroCollector {
    fn mark_bare_unresolved(&mut self) {
        self.authority.bare = OutputMacroResolution::Unresolved;
    }

    fn mark_all_unresolved(&mut self) {
        self.authority.bare = OutputMacroResolution::Unresolved;
        self.authority.standard = OutputMacroResolution::Unresolved;
    }

    fn collect_use(&mut self, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => self.collect_use(&path.tree),
            syn::UseTree::Name(name) => self.mark_bound(&ident_name(&name.ident)),
            syn::UseTree::Rename(rename) => self.mark_bound(&ident_name(&rename.rename)),
            syn::UseTree::Glob(_) => self.mark_all_unresolved(),
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.collect_use(tree);
                }
            }
        }
    }

    fn mark_bound(&mut self, name: &str) {
        match name {
            "println" => self.mark_bare_unresolved(),
            "std" => self.mark_all_unresolved(),
            _ => {}
        }
    }
}

impl<'ast> Visit<'ast> for OutputMacroCollector {
    fn visit_item(&mut self, item: &'ast Item) {
        if item_attributes(item)
            .iter()
            .any(|attribute| attribute.path().is_ident("macro_use"))
        {
            self.mark_bare_unresolved();
        }
        if item_attributes(item).iter().any(attribute_may_expand_items) {
            self.mark_all_unresolved();
        }
        match item {
            Item::ExternCrate(item) => {
                let bound = item
                    .rename
                    .as_ref()
                    .map_or_else(|| ident_name(&item.ident), |(_, name)| ident_name(name));
                self.mark_bound(&bound);
            }
            Item::Macro(item) => {
                if item.mac.path.is_ident("macro_rules") {
                    if item.ident.as_ref().is_some_and(|name| name == "println") {
                        self.mark_bare_unresolved();
                    }
                } else {
                    self.mark_all_unresolved();
                }
            }
            Item::Mod(item) => self.mark_bound(&ident_name(&item.ident)),
            Item::Use(item) => self.collect_use(&item.tree),
            _ => {}
        }
        syn::visit::visit_item(self, item);
    }
}
