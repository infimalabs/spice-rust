//! One tree-sitter structural scan path over every supported source grammar.

use std::error::Error;
use std::fmt;

use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};
use tree_sitter_language::LanguageFn;

/// One language-independent structural pattern identity.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StructuralPattern {
    /// A direct call whose callee is one identifier.
    FunctionCall,
    /// One named JavaScript declaration.
    JavaScriptDeclaration,
    /// One JavaScript identifier occurrence.
    JavaScriptIdentifier,
}

impl StructuralPattern {
    /// Stable pattern name used in diagnostics and evidence.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FunctionCall => "function-call",
            Self::JavaScriptDeclaration => "javascript-declaration",
            Self::JavaScriptIdentifier => "javascript-identifier",
        }
    }
}

/// One named capture retained from a structural match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralCapture {
    name: String,
    text: String,
    start_byte: usize,
    end_byte: usize,
}

impl StructuralCapture {
    /// Query capture name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Exact source text selected by the capture.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Inclusive byte offset where the capture begins.
    #[must_use]
    pub const fn start_byte(&self) -> usize {
        self.start_byte
    }

    /// Exclusive byte offset where the capture ends.
    #[must_use]
    pub const fn end_byte(&self) -> usize {
        self.end_byte
    }
}

/// One language-independent structural pattern match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralMatch {
    pattern: StructuralPattern,
    start_byte: usize,
    end_byte: usize,
    captures: Vec<StructuralCapture>,
}

impl StructuralMatch {
    /// Pattern that matched.
    #[must_use]
    pub const fn pattern(&self) -> StructuralPattern {
        self.pattern
    }

    /// Inclusive byte offset where the complete match begins.
    #[must_use]
    pub const fn start_byte(&self) -> usize {
        self.start_byte
    }

    /// Exclusive byte offset where the complete match ends.
    #[must_use]
    pub const fn end_byte(&self) -> usize {
        self.end_byte
    }

    /// Named captures retained in source order.
    #[must_use]
    pub fn captures(&self) -> &[StructuralCapture] {
        &self.captures
    }
}

/// Typed reason a structural scan could not produce matches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StructuralScanError {
    /// The caller named no grammar in the scanner's closed registry.
    UnsupportedLanguage {
        observed: String,
        supported: Vec<&'static str>,
    },
    /// A packaged grammar cannot be installed in the current parser.
    Grammar {
        language: &'static str,
        reason: String,
    },
    /// Tree-sitter could not produce one syntax tree.
    ParseStopped { language: &'static str },
    /// The source contains syntax errors under the selected grammar.
    MalformedSource { language: &'static str },
    /// A compiled pattern declaration and grammar binding disagree.
    PatternBinding {
        language: &'static str,
        pattern: StructuralPattern,
        reason: String,
    },
}

impl fmt::Display for StructuralScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedLanguage {
                observed,
                supported,
            } => write!(
                formatter,
                "unsupported structural-scan language {observed:?}; choose {}",
                supported.join(", ")
            ),
            Self::Grammar { language, reason } => {
                write!(
                    formatter,
                    "{language} structural grammar is incompatible: {reason}"
                )
            }
            Self::ParseStopped { language } => {
                write!(
                    formatter,
                    "{language} structural parse stopped without a tree"
                )
            }
            Self::MalformedSource { language } => {
                write!(
                    formatter,
                    "source is malformed under the {language} grammar"
                )
            }
            Self::PatternBinding {
                language,
                pattern,
                reason,
            } => write!(
                formatter,
                "{language} binding for structural pattern {} is invalid: {reason}",
                pattern.as_str()
            ),
        }
    }
}

impl Error for StructuralScanError {}

#[derive(Clone, Copy)]
struct PatternDeclaration {
    pattern: StructuralPattern,
    match_capture: &'static str,
}

#[derive(Clone, Copy)]
struct PatternBinding {
    pattern: StructuralPattern,
    query: &'static str,
}

#[derive(Clone, Copy)]
struct GrammarBinding {
    name: &'static str,
    language: LanguageFn,
    patterns: &'static [PatternBinding],
}

const PATTERNS: &[PatternDeclaration] = &[PatternDeclaration {
    pattern: StructuralPattern::FunctionCall,
    match_capture: "match",
}];

const JAVASCRIPT_SYMBOL_PATTERNS: &[PatternDeclaration] = &[
    PatternDeclaration {
        pattern: StructuralPattern::JavaScriptDeclaration,
        match_capture: "match",
    },
    PatternDeclaration {
        pattern: StructuralPattern::JavaScriptIdentifier,
        match_capture: "match",
    },
];

const JAVASCRIPT_SYMBOL_BINDINGS: &[PatternBinding] = &[
    PatternBinding {
        pattern: StructuralPattern::JavaScriptDeclaration,
        query: concat!(
            "[",
            " (class_declaration name: (identifier) @class)",
            " (function_declaration name: (identifier) @function)",
            " (generator_function_declaration name: (identifier) @function)",
            " (lexical_declaration \"const\" ",
            "(variable_declarator name: (identifier) @constant))",
            " (lexical_declaration \"let\" ",
            "(variable_declarator name: (identifier) @let))",
            " (variable_declaration ",
            "(variable_declarator name: (identifier) @variable))",
            "] @match",
        ),
    },
    PatternBinding {
        pattern: StructuralPattern::JavaScriptIdentifier,
        query: "(identifier) @symbol @match",
    },
];

const GRAMMARS: &[GrammarBinding] = &[
    GrammarBinding {
        name: "python",
        language: tree_sitter_python::LANGUAGE,
        patterns: &[PatternBinding {
            pattern: StructuralPattern::FunctionCall,
            query: "(call function: (identifier) @callee) @match",
        }],
    },
    GrammarBinding {
        name: "rust",
        language: tree_sitter_rust::LANGUAGE,
        patterns: &[PatternBinding {
            pattern: StructuralPattern::FunctionCall,
            query: "(call_expression function: (identifier) @callee) @match",
        }],
    },
    GrammarBinding {
        name: "javascript",
        language: tree_sitter_javascript::LANGUAGE,
        patterns: &[PatternBinding {
            pattern: StructuralPattern::FunctionCall,
            query: "(call_expression function: (identifier) @callee) @match",
        }],
    },
    GrammarBinding {
        name: "csharp",
        language: tree_sitter_c_sharp::LANGUAGE,
        patterns: &[PatternBinding {
            pattern: StructuralPattern::FunctionCall,
            query: "(invocation_expression function: (identifier) @callee) @match",
        }],
    },
    GrammarBinding {
        name: "lua",
        language: tree_sitter_lua::LANGUAGE,
        patterns: &[PatternBinding {
            pattern: StructuralPattern::FunctionCall,
            query: "(function_call name: (identifier) @callee) @match",
        }],
    },
];

/// Parse source with one supported grammar and run every declared structural pattern.
///
/// Language selection, parsing, query execution, and result normalization all
/// occur in this entrypoint. Grammar-specific knowledge is confined to
/// [`GRAMMARS`].
///
/// # Errors
///
/// Returns [`StructuralScanError::UnsupportedLanguage`] when `language` does
/// not name one of `python`, `rust`, `javascript`, `csharp`, or `lua`. Other
/// variants report packaged grammar, source, or pattern-contract failures.
pub fn scan_structural(
    language: &str,
    source: &str,
) -> Result<Vec<StructuralMatch>, StructuralScanError> {
    let grammar = grammar(language)?;
    scan(grammar, source, PATTERNS, grammar.patterns)
}

/// Parse JavaScript once and retain its named declarations and identifiers.
///
/// This stays crate-internal because it is one domain-specific query set over
/// the shared structural scanner, not a second public scanner contract.
pub(crate) fn scan_javascript_symbols(
    source: &str,
) -> Result<Vec<StructuralMatch>, StructuralScanError> {
    let grammar = grammar("javascript")?;
    scan(
        grammar,
        source,
        JAVASCRIPT_SYMBOL_PATTERNS,
        JAVASCRIPT_SYMBOL_BINDINGS,
    )
}

fn grammar(language: &str) -> Result<&'static GrammarBinding, StructuralScanError> {
    GRAMMARS
        .iter()
        .find(|binding| binding.name == language)
        .ok_or_else(|| StructuralScanError::UnsupportedLanguage {
            observed: language.to_owned(),
            supported: GRAMMARS.iter().map(|binding| binding.name).collect(),
        })
}

fn scan(
    grammar: &GrammarBinding,
    source: &str,
    declarations: &[PatternDeclaration],
    bindings: &[PatternBinding],
) -> Result<Vec<StructuralMatch>, StructuralScanError> {
    let language: Language = grammar.language.into();
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|error| StructuralScanError::Grammar {
            language: grammar.name,
            reason: error.to_string(),
        })?;
    let tree = parser
        .parse(source, None)
        .ok_or(StructuralScanError::ParseStopped {
            language: grammar.name,
        })?;
    if tree.root_node().has_error() {
        return Err(StructuralScanError::MalformedSource {
            language: grammar.name,
        });
    }

    let mut findings = Vec::new();
    for declaration in declarations {
        scan_pattern(
            grammar,
            bindings,
            &language,
            tree.root_node(),
            source,
            *declaration,
            &mut findings,
        )?;
    }
    findings.sort_by_key(|finding| (finding.start_byte, finding.end_byte, finding.pattern));
    Ok(findings)
}

fn scan_pattern(
    grammar: &GrammarBinding,
    bindings: &[PatternBinding],
    language: &Language,
    root: tree_sitter::Node<'_>,
    source: &str,
    declaration: PatternDeclaration,
    findings: &mut Vec<StructuralMatch>,
) -> Result<(), StructuralScanError> {
    let binding = bindings
        .iter()
        .find(|binding| binding.pattern == declaration.pattern)
        .ok_or_else(|| binding_error(grammar, declaration, "query is missing"))?;
    let query = Query::new(language, binding.query).map_err(|error| {
        binding_error(
            grammar,
            declaration,
            format!("query did not compile: {error}"),
        )
    })?;
    let match_capture = query
        .capture_index_for_name(declaration.match_capture)
        .ok_or_else(|| binding_error(grammar, declaration, "match capture is missing"))?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, root, source.as_bytes());
    while let Some(found) = matches.next() {
        let complete = found
            .captures
            .iter()
            .find(|capture| capture.index == match_capture)
            .ok_or_else(|| {
                binding_error(grammar, declaration, "query omitted its match capture")
            })?;
        let complete_range = complete.node.byte_range();
        let mut captures = found
            .captures
            .iter()
            .filter(|capture| capture.index != match_capture)
            .map(|capture| {
                let range = capture.node.byte_range();
                let text = source.get(range.clone()).ok_or_else(|| {
                    binding_error(
                        grammar,
                        declaration,
                        "capture is not on UTF-8 source boundaries",
                    )
                })?;
                Ok(StructuralCapture {
                    name: query.capture_names()[capture.index as usize].to_owned(),
                    text: text.to_owned(),
                    start_byte: range.start,
                    end_byte: range.end,
                })
            })
            .collect::<Result<Vec<_>, StructuralScanError>>()?;
        captures.sort_by(|left, right| {
            (left.start_byte, left.end_byte, &left.name).cmp(&(
                right.start_byte,
                right.end_byte,
                &right.name,
            ))
        });
        findings.push(StructuralMatch {
            pattern: declaration.pattern,
            start_byte: complete_range.start,
            end_byte: complete_range.end,
            captures,
        });
    }
    Ok(())
}

fn binding_error(
    grammar: &GrammarBinding,
    declaration: PatternDeclaration,
    reason: impl Into<String>,
) -> StructuralScanError {
    StructuralScanError::PatternBinding {
        language: grammar.name,
        pattern: declaration.pattern,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_function_call_pattern_has_the_same_result_shape_in_every_grammar() {
        let fixtures = [
            ("python", "def run():\n    audit(\"fixture\")\n"),
            ("rust", "fn run() { audit(\"fixture\"); }\n"),
            ("javascript", "function run() { audit(\"fixture\"); }\n"),
            (
                "csharp",
                "class Fixture { void Run() { audit(\"fixture\"); } }\n",
            ),
            ("lua", "function run()\n  audit(\"fixture\")\nend\n"),
        ];

        for (language, source) in fixtures {
            let matches = scan_structural(language, source).expect("fixture scans");
            let [matched] = matches.as_slice() else {
                panic!("{language} produced unexpected matches: {matches:?}");
            };
            assert_eq!(matched.pattern(), StructuralPattern::FunctionCall);
            assert_eq!(
                source
                    .get(matched.start_byte()..matched.end_byte())
                    .expect("match uses source boundaries"),
                "audit(\"fixture\")",
            );
            let [callee] = matched.captures() else {
                panic!("{language} produced unexpected captures: {matched:?}");
            };
            assert_eq!(callee.name(), "callee");
            assert_eq!(callee.text(), "audit");
            assert_eq!(
                source
                    .get(callee.start_byte()..callee.end_byte())
                    .expect("capture uses source boundaries"),
                "audit",
            );
        }
    }

    #[test]
    fn javascript_symbol_scan_retains_every_supported_declaration_kind() {
        let source = concat!(
            "class Fixture {}\n",
            "function run() {}\n",
            "function* stream() {}\n",
            "const fixed = 1;\n",
            "let mutable = 2;\n",
            "var legacy = 3;\n",
            "run();\n",
        );
        let matches = scan_javascript_symbols(source).expect("JavaScript symbols scan");
        let declarations: Vec<(&str, &str)> = matches
            .iter()
            .filter(|matched| matched.pattern() == StructuralPattern::JavaScriptDeclaration)
            .map(|matched| {
                let [capture] = matched.captures() else {
                    panic!("declaration has unexpected captures: {matched:?}");
                };
                (capture.name(), capture.text())
            })
            .collect();
        assert_eq!(
            declarations,
            [
                ("class", "Fixture"),
                ("function", "run"),
                ("function", "stream"),
                ("constant", "fixed"),
                ("let", "mutable"),
                ("variable", "legacy"),
            ]
        );
        assert_eq!(
            matches
                .iter()
                .filter(|matched| {
                    matched.pattern() == StructuralPattern::JavaScriptIdentifier
                        && matched
                            .captures()
                            .iter()
                            .any(|capture| capture.text() == "run")
                })
                .count(),
            2,
            "the declaration and call are distinct identifier occurrences"
        );
    }

    #[test]
    fn every_grammar_binds_every_pattern_once() {
        for grammar in GRAMMARS {
            assert_eq!(grammar.patterns.len(), PATTERNS.len(), "{}", grammar.name);
            for declaration in PATTERNS {
                assert_eq!(
                    grammar
                        .patterns
                        .iter()
                        .filter(|binding| binding.pattern == declaration.pattern)
                        .count(),
                    1,
                    "{} {}",
                    grammar.name,
                    declaration.pattern.as_str(),
                );
            }
        }
    }

    #[test]
    fn unsupported_language_is_a_typed_refusal_not_an_empty_scan() {
        let error =
            scan_structural("go", "audit(\"fixture\")").expect_err("unsupported grammar refuses");
        assert_eq!(
            error,
            StructuralScanError::UnsupportedLanguage {
                observed: "go".to_owned(),
                supported: vec!["python", "rust", "javascript", "csharp", "lua"],
            }
        );
        assert_eq!(
            error.to_string(),
            concat!(
                "unsupported structural-scan language \"go\"; choose ",
                "python, rust, javascript, csharp, lua",
            )
        );
    }
}
