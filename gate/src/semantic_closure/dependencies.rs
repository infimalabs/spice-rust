//! Static repository-local language dependency resolution.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use tree_sitter::Node;

use super::{GateDependencyMode, GateSourceDependency};

pub(super) fn dependency_edges(
    path: &str,
    root: Node<'_>,
    bytes: &[u8],
    paths: &BTreeSet<String>,
) -> (Vec<GateSourceDependency>, Option<String>) {
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let mut dependencies = Vec::new();
    let mut gap = None;
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        let text = node.utf8_text(bytes).unwrap_or_default();
        let edge = match extension {
            "rs" => rust_dependency(path, node.kind(), text, paths),
            "js" | "mjs" | "cjs" | "jsx" => javascript_dependency(path, node.kind(), text, paths),
            "py" | "pyi" => python_dependency(path, node.kind(), text),
            "cs" => csharp_dependency(path, node.kind(), text, paths),
            "lua" => lua_dependency(path, node.kind(), text, paths),
            _ => DependencyEdge::None,
        };
        match edge {
            DependencyEdge::Resolved { path, mode } => {
                dependencies.push(GateSourceDependency { path, mode });
            }
            DependencyEdge::Gap(detail) => {
                if gap.is_none() {
                    gap = Some(detail);
                }
            }
            DependencyEdge::None => {}
        }
        let mut cursor = node.walk();
        pending.extend(node.children(&mut cursor));
    }
    (dependencies, gap)
}

enum DependencyEdge {
    None,
    Resolved {
        path: String,
        mode: GateDependencyMode,
    },
    Gap(String),
}

fn rust_dependency(path: &str, kind: &str, text: &str, paths: &BTreeSet<String>) -> DependencyEdge {
    if kind == "macro_invocation" {
        let Some((name, arguments)) = text.split_once('!') else {
            return DependencyEdge::None;
        };
        let name = name.trim();
        if !matches!(name, "include" | "include_str" | "include_bytes") {
            return DependencyEdge::None;
        }
        let Some(specifier) = parenthesized_literal(arguments) else {
            return DependencyEdge::Gap(format!(
                "dynamic Rust {name}! dependency in {path:?} cannot be proven",
            ));
        };
        let mode = if name == "include" {
            GateDependencyMode::Syntax
        } else {
            GateDependencyMode::Exact
        };
        return resolve_exact_relative(path, &specifier, paths).map_or_else(
            || {
                DependencyEdge::Gap(format!(
                    "Rust {name}! dependency {specifier:?} from {path:?} is unresolved",
                ))
            },
            |path| DependencyEdge::Resolved { path, mode },
        );
    }
    if kind != "mod_item" || !text.trim_end().ends_with(';') {
        return DependencyEdge::None;
    }
    match rust_path_attribute(text) {
        Some(Ok(specifier)) => {
            return resolve_exact_relative(path, &specifier, paths).map_or_else(
                || {
                    DependencyEdge::Gap(format!(
                        "Rust #[path] dependency {specifier:?} from {path:?} is unresolved",
                    ))
                },
                |path| DependencyEdge::Resolved {
                    path,
                    mode: GateDependencyMode::Syntax,
                },
            );
        }
        Some(Err(())) => {
            return DependencyEdge::Gap(format!(
                "dynamic Rust #[path] dependency in {path:?} cannot be proven",
            ));
        }
        None => {}
    }
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    let Some(index) = tokens.iter().position(|token| *token == "mod") else {
        return DependencyEdge::None;
    };
    let Some(name) = tokens.get(index + 1).map(|name| name.trim_end_matches(';')) else {
        return DependencyEdge::Gap(format!("Rust module dependency in {path:?} is incomplete"));
    };
    let source = Path::new(path);
    let parent = source.parent().unwrap_or_else(|| Path::new(""));
    let file = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let base = if matches!(file, "lib.rs" | "main.rs" | "mod.rs" | "build.rs") {
        parent.to_path_buf()
    } else {
        parent.join(source.file_stem().unwrap_or_default())
    };
    let candidates = [
        base.join(format!("{name}.rs")),
        base.join(name).join("mod.rs"),
    ];
    resolve_candidates(candidates, paths).map_or_else(
        || {
            DependencyEdge::Gap(format!(
                "Rust module {name:?} from {path:?} has no one selected source",
            ))
        },
        |path| DependencyEdge::Resolved {
            path,
            mode: GateDependencyMode::Syntax,
        },
    )
}

fn rust_path_attribute(text: &str) -> Option<Result<String, ()>> {
    let mut rest = text;
    while let Some(start) = rest.find("#[") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find(']') else {
            return Some(Err(()));
        };
        let attribute = &rest[..end];
        let trimmed = attribute.trim_start();
        if let Some(value) = trimmed.strip_prefix("path").filter(|value| {
            value
                .chars()
                .next()
                .is_none_or(|next| next.is_ascii_whitespace() || next == '=')
        }) {
            let Some(value) = value.trim_start().strip_prefix('=') else {
                return Some(Err(()));
            };
            let value = value.trim_start();
            return Some(
                leading_literal(value)
                    .map(|(specifier, _)| specifier)
                    .ok_or(()),
            );
        }
        if attribute
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .any(|token| token == "path")
        {
            return Some(Err(()));
        }
        rest = &rest[end + 1..];
    }
    None
}

fn javascript_dependency(
    path: &str,
    kind: &str,
    text: &str,
    paths: &BTreeSet<String>,
) -> DependencyEdge {
    let specifier = if matches!(kind, "import_statement" | "export_statement") {
        last_literal(text)
    } else if kind == "call_expression"
        && text
            .split_once('(')
            .is_some_and(|(callee, _)| matches!(callee.trim(), "import" | "require"))
    {
        parenthesized_literal(
            text.split_once('(')
                .map(|(_, arguments)| format!("({arguments}"))
                .as_deref()
                .unwrap_or_default(),
        )
    } else {
        return DependencyEdge::None;
    };
    let Some(specifier) = specifier else {
        return DependencyEdge::Gap(format!(
            "dynamic JavaScript module dependency in {path:?} cannot be proven",
        ));
    };
    if !specifier.starts_with('.') {
        return DependencyEdge::None;
    }
    resolve_javascript(path, &specifier, paths).map_or_else(
        || {
            DependencyEdge::Gap(format!(
                "JavaScript dependency {specifier:?} from {path:?} is unresolved",
            ))
        },
        |path| DependencyEdge::Resolved {
            mode: if path.ends_with(".json") {
                GateDependencyMode::Exact
            } else {
                GateDependencyMode::Syntax
            },
            path,
        },
    )
}

fn python_dependency(path: &str, kind: &str, text: &str) -> DependencyEdge {
    if kind != "call" {
        return DependencyEdge::None;
    }
    let Some((callee, arguments)) = text.split_once('(') else {
        return DependencyEdge::None;
    };
    if !matches!(callee.trim(), "__import__" | "importlib.import_module") {
        return DependencyEdge::None;
    }
    if parenthesized_literal(&format!("({arguments}")).is_some() {
        DependencyEdge::None
    } else {
        DependencyEdge::Gap(format!(
            "dynamic Python module dependency in {path:?} cannot be proven",
        ))
    }
}

fn csharp_dependency(
    path: &str,
    kind: &str,
    text: &str,
    paths: &BTreeSet<String>,
) -> DependencyEdge {
    if kind != "preproc_load" && !text.trim_start().starts_with("#load") {
        return DependencyEdge::None;
    }
    let Some((specifier, _)) = leading_literal(text.trim_start_matches("#load").trim()) else {
        return DependencyEdge::Gap(format!(
            "dynamic C# #load dependency in {path:?} cannot be proven",
        ));
    };
    resolve_exact_relative(path, &specifier, paths).map_or_else(
        || {
            DependencyEdge::Gap(format!(
                "C# #load dependency {specifier:?} from {path:?} is unresolved",
            ))
        },
        |path| DependencyEdge::Resolved {
            path,
            mode: GateDependencyMode::Syntax,
        },
    )
}

fn lua_dependency(path: &str, kind: &str, text: &str, paths: &BTreeSet<String>) -> DependencyEdge {
    if kind != "function_call" || !text.trim_start().starts_with("require") {
        return DependencyEdge::None;
    }
    let Some(specifier) = parenthesized_literal(text.trim_start_matches("require").trim()) else {
        return DependencyEdge::Gap(format!(
            "dynamic Lua require dependency in {path:?} cannot be proven",
        ));
    };
    let relative = format!("{}.lua", specifier.replace('.', "/"));
    resolve_exact_relative(path, &relative, paths).map_or_else(
        || {
            DependencyEdge::Gap(format!(
                "Lua dependency {specifier:?} from {path:?} is unresolved",
            ))
        },
        |path| DependencyEdge::Resolved {
            path,
            mode: GateDependencyMode::Syntax,
        },
    )
}

fn resolve_javascript(path: &str, specifier: &str, paths: &BTreeSet<String>) -> Option<String> {
    let exact = normalized_relative(path, specifier)?;
    let candidates = [
        PathBuf::from(&exact),
        PathBuf::from(format!("{exact}.js")),
        PathBuf::from(format!("{exact}.mjs")),
        PathBuf::from(format!("{exact}.cjs")),
        PathBuf::from(format!("{exact}.json")),
        PathBuf::from(&exact).join("index.js"),
        PathBuf::from(&exact).join("index.mjs"),
        PathBuf::from(&exact).join("index.cjs"),
    ];
    resolve_candidates(candidates, paths)
}

fn resolve_exact_relative(path: &str, specifier: &str, paths: &BTreeSet<String>) -> Option<String> {
    let normalized = normalized_relative(path, specifier)?;
    paths.contains(&normalized).then_some(normalized)
}

fn normalized_relative(path: &str, specifier: &str) -> Option<String> {
    let joined = Path::new(path)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(specifier);
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(component) => normalized.push(component),
            Component::ParentDir if normalized.pop() => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    normalized.to_str().map(|path| path.replace('\\', "/"))
}

fn resolve_candidates(
    candidates: impl IntoIterator<Item = PathBuf>,
    paths: &BTreeSet<String>,
) -> Option<String> {
    let matching = candidates
        .into_iter()
        .filter_map(|candidate| candidate.to_str().map(|path| path.replace('\\', "/")))
        .filter(|candidate| paths.contains(candidate))
        .collect::<Vec<_>>();
    (matching.len() == 1).then(|| matching[0].clone())
}

fn parenthesized_literal(text: &str) -> Option<String> {
    let text = text.trim();
    let text = text.strip_prefix('(')?.strip_suffix(')')?.trim();
    let (literal, rest) = leading_literal(text)?;
    rest.trim()
        .trim_end_matches(',')
        .trim()
        .is_empty()
        .then_some(literal)
}

fn last_literal(text: &str) -> Option<String> {
    let mut literals = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let Some(position) = rest.find(['\'', '"']) else {
            break;
        };
        rest = &rest[position..];
        let Some((literal, tail)) = leading_literal(rest) else {
            rest = &rest[1..];
            continue;
        };
        literals.push(literal);
        rest = tail;
    }
    literals.pop()
}

fn leading_literal(text: &str) -> Option<(String, &str)> {
    let text = text.trim_start();
    let quote = text.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let mut escaped = false;
    for (index, byte) in text.as_bytes().iter().copied().enumerate().skip(1) {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == quote {
            let literal = text[1..index].replace("\\\"", "\"").replace("\\'", "'");
            return Some((literal, &text[index + 1..]));
        }
    }
    None
}
