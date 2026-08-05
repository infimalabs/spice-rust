use std::collections::BTreeSet;
use std::path::Path;

use tree_sitter::{Language, Node, Parser};
use tree_sitter_language::LanguageFn;

use super::{ParseError, canonical_path};

/// One named routine and the two quantities the complexity arm measures on it.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ComplexityRoutine {
    path: String,
    function: String,
    ccn: u64,
    length: u64,
}

impl ComplexityRoutine {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The routine's name qualified by every named scope enclosing it.
    ///
    /// Qualified rather than bare because the bare name is not unique in any of
    /// these languages: two Rust impl blocks each declaring `new`, or two
    /// classes each declaring `run`, are ordinary. A metric keyed on
    /// path-and-function would collide on the first such file and refuse it.
    #[must_use]
    pub fn function(&self) -> &str {
        &self.function
    }

    /// Cyclomatic complexity: one, plus every decision written inside.
    #[must_use]
    pub const fn ccn(&self) -> u64 {
        self.ccn
    }

    /// Lines the routine spans, first through last inclusive.
    #[must_use]
    pub const fn length(&self) -> u64 {
        self.length
    }
}

/// One grammar's answers to the three questions this measurement asks: what is a
/// routine, what names a routine's scope, and what is a decision.
///
/// Every entry is a node kind taken from the vendored grammar itself rather than
/// a token spelling, so a construct written in an unusual but legal way is
/// counted the same as the ordinary form.
struct ComplexityGrammar {
    suffixes: &'static [&'static str],
    language: LanguageFn,
    /// Node kinds that introduce one routine bearing a declared name.
    routines: &'static [&'static str],
    /// Node kinds that name a scope the routines inside them are qualified by.
    scopes: &'static [&'static str],
    /// Node kinds that each add exactly one decision.
    decisions: &'static [&'static str],
    /// Node kinds carrying an infix operator, tested against `short_circuit`.
    infix: &'static [&'static str],
    /// Operator spellings that add one decision because they short-circuit.
    short_circuit: &'static [&'static str],
}

/// Every alternative of a multi-way branch is counted, including the default
/// one, because a written alternative is a path a reader has to follow whether
/// or not the grammar gives it a distinct node kind. This is also the only rule
/// that reads the same in all five grammars: Rust spells its alternatives
/// `match_arm`, JavaScript `switch_case` beside `switch_default`, and C# groups
/// them into `switch_section`, and no uniform statement about defaults survives
/// all three.
const GRAMMARS: &[ComplexityGrammar] = &[
    ComplexityGrammar {
        suffixes: &[".rs"],
        language: tree_sitter_rust::LANGUAGE,
        routines: &["function_item"],
        scopes: &["impl_item", "trait_item", "mod_item"],
        decisions: &[
            "if_expression",
            "while_expression",
            "for_expression",
            "match_arm",
            "try_expression",
        ],
        infix: &["binary_expression"],
        short_circuit: &["&&", "||"],
    },
    ComplexityGrammar {
        suffixes: &[".py"],
        language: tree_sitter_python::LANGUAGE,
        routines: &["function_definition"],
        scopes: &["class_definition"],
        decisions: &[
            "if_statement",
            "elif_clause",
            "while_statement",
            "for_statement",
            "except_clause",
            "case_clause",
            "boolean_operator",
            "conditional_expression",
            "if_clause",
        ],
        infix: &[],
        short_circuit: &[],
    },
    ComplexityGrammar {
        suffixes: &[".js", ".mjs"],
        language: tree_sitter_javascript::LANGUAGE,
        routines: &["function_declaration", "method_definition"],
        scopes: &["class_declaration"],
        decisions: &[
            "if_statement",
            "while_statement",
            "do_statement",
            "for_statement",
            "for_in_statement",
            "switch_case",
            "switch_default",
            "catch_clause",
            "ternary_expression",
        ],
        infix: &["binary_expression"],
        short_circuit: &["&&", "||", "??"],
    },
    ComplexityGrammar {
        suffixes: &[".cs"],
        language: tree_sitter_c_sharp::LANGUAGE,
        routines: &["method_declaration", "local_function_statement"],
        scopes: &[
            "namespace_declaration",
            "class_declaration",
            "struct_declaration",
            "interface_declaration",
            "record_declaration",
        ],
        decisions: &[
            "if_statement",
            "while_statement",
            "do_statement",
            "for_statement",
            "foreach_statement",
            "switch_section",
            "catch_clause",
            "conditional_expression",
        ],
        infix: &["binary_expression"],
        short_circuit: &["&&", "||", "??"],
    },
    ComplexityGrammar {
        suffixes: &[".lua"],
        language: tree_sitter_lua::LANGUAGE,
        routines: &["function_declaration"],
        scopes: &[],
        decisions: &[
            "if_statement",
            "elseif_statement",
            "while_statement",
            "for_statement",
            "repeat_statement",
        ],
        infix: &["binary_expression"],
        short_circuit: &["and", "or"],
    },
];

/// Measure every named routine one selected source file declares.
///
/// # Errors
///
/// Refuses a source class with no vendored grammar as
/// [`ParseError::Unsupported`], so a language the policy selects and this build
/// cannot parse is named rather than measured as having no routines at all.
/// Refuses source the grammar cannot parse, and refuses two routines that
/// qualify to one identity, since the arm keys its measurements on that pair.
pub fn measure_complexity(path: &str, source: &str) -> Result<Vec<ComplexityRoutine>, ParseError> {
    let path = canonical_path(path)?;
    let grammar = grammar_for(&path)?;
    let mut parser = Parser::new();
    parser
        .set_language(&Language::new(grammar.language))
        .map_err(|error| {
            ParseError::malformed(
                format!("complexity grammar for {path:?}"),
                error.to_string(),
            )
        })?;
    let tree = parser.parse(source, None).ok_or_else(|| {
        ParseError::malformed(
            format!("complexity source {path:?}"),
            "grammar produced no tree",
        )
    })?;
    let mut routines = Vec::new();
    let mut scope = Vec::new();
    visit(
        tree.root_node(),
        source,
        grammar,
        &path,
        &mut scope,
        None,
        &mut routines,
    );
    let mut seen = BTreeSet::new();
    for routine in &routines {
        if !seen.insert(routine.function.clone()) {
            return Err(ParseError::collision(
                format!("{}:{}", routine.path, routine.function),
                "two routines in this file qualify to one identity",
            ));
        }
    }
    routines.sort();
    Ok(routines)
}

/// The vendored grammar for one selected path's source class.
fn grammar_for(path: &str) -> Result<&'static ComplexityGrammar, ParseError> {
    let suffix = Path::new(path)
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default();
    GRAMMARS
        .iter()
        .find(|grammar| grammar.suffixes.contains(&suffix.as_str()))
        .ok_or_else(|| {
            ParseError::unsupported(
                "policy.languages.complexity",
                format!("source class {suffix:?} has no vendored tree-sitter grammar"),
                "vendor a grammar for this source class or remove it from complexity scope",
            )
        })
}

/// Walk one subtree, attributing each decision to the routine that encloses it.
///
/// `current` is the innermost routine seen on the way down, so a decision inside
/// a nested routine counts for the nested one alone. A decision written outside
/// every routine — a module-level branch — is counted by nothing, because the
/// arm measures routines and there is none to hold it.
fn visit(
    node: Node<'_>,
    source: &str,
    grammar: &ComplexityGrammar,
    path: &str,
    scope: &mut Vec<String>,
    current: Option<usize>,
    routines: &mut Vec<ComplexityRoutine>,
) {
    let mut current = current;
    let mut named = false;
    if grammar.routines.contains(&node.kind()) {
        if let Some(name) = declared_name(node, source) {
            scope.push(name);
            named = true;
            routines.push(ComplexityRoutine {
                path: path.to_owned(),
                function: scope.join("::"),
                ccn: 1,
                length: span_lines(node),
            });
            current = Some(routines.len() - 1);
        }
    } else if grammar.scopes.contains(&node.kind()) {
        if let Some(name) = declared_name(node, source) {
            scope.push(name);
            named = true;
        }
    } else if is_decision(node, grammar)
        && let Some(index) = current
    {
        routines[index].ccn += 1;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit(child, source, grammar, path, scope, current, routines);
    }
    if named {
        scope.pop();
    }
}

/// Whether one node adds a decision to the routine holding it.
fn is_decision(node: Node<'_>, grammar: &ComplexityGrammar) -> bool {
    if grammar.decisions.contains(&node.kind()) {
        return true;
    }
    if !grammar.infix.contains(&node.kind()) {
        return false;
    }
    // An operator token's node kind is its own spelling, so the operator is read
    // from the tree rather than from the source text a field name would locate.
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| grammar.short_circuit.contains(&child.kind()))
}

/// The name a declaration states, from whichever field this grammar puts it in.
///
/// Rust names an impl block by its `type` rather than a `name`, and an
/// unnameable declaration — an anonymous function, an impl of a type this walk
/// cannot spell — yields nothing and is not measured, because there would be no
/// identity to key its measurement on.
fn declared_name(node: Node<'_>, source: &str) -> Option<String> {
    ["name", "type"]
        .into_iter()
        .find_map(|field| node.child_by_field_name(field))
        .and_then(|child| child.utf8_text(source.as_bytes()).ok())
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.contains(['\r', '\n']))
        .map(str::to_owned)
}

/// Lines one node spans, first through last inclusive.
fn span_lines(node: Node<'_>) -> u64 {
    let start = node.start_position().row;
    let end = node.end_position().row;
    (end.saturating_sub(start) as u64) + 1
}
