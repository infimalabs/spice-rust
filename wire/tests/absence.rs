//! The one spelling for absent optional fields.
//!
//! Literal documents prove the rule holds for the fields that exist. They
//! cannot reach the field nobody has written yet, and the rule is otherwise
//! prose plus a hand-applied attribute triple, so the next optional field can
//! omit one attribute and admit a second spelling of absence with the suite
//! green. The source check below reads every crossing declaration and requires
//! the triple, which is the enforcement every other law in this crate already
//! has.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use syn::punctuated::Punctuated;
use syn::{Meta, Token};
use wire::{CursorHead, LaneResidency, Packed, Refusal, WorkflowHead};

const WORKFLOW: &str = r#"{
  "flow": ["todo", "review"],
  "phase": "todo",
  "status": "pending",
  "review": "unassigned"
}"#;

const LANE: &str = r#"{
  "lane": "018f82d4-6f59-7d06-92e3-bce913116302",
  "lineage_id": "018f82d4-6f59-7d06-92e3-bce913116302",
  "worktree": "/worktrees/lane-a",
  "life": "drive",
  "vendor": "claude",
  "lineage": [],
  "capabilities": [],
  "disposition": "vacant"
}"#;

const CURSOR: &str = r#"{}"#;

const PACKED: &str = r#"{
  "rows": [],
  "horizon": 48213
}"#;

const REFUSAL: &str = r#"{
  "reason": "claim::not-held",
  "remedy": "reclaim the task"
}"#;

#[test]
fn absent_head_fields_are_omitted_and_null_is_refused() {
    assert_canonical_absence::<WorkflowHead>(WORKFLOW, &["assignment", "deferral", "completion"]);
    assert_canonical_absence::<LaneResidency>(LANE, &["occupant", "session"]);
    assert_canonical_absence::<CursorHead>(CURSOR, &["seq", "wake"]);
}

#[test]
fn absent_non_head_fields_are_omitted_and_null_is_refused() {
    assert_canonical_absence::<Packed<serde_json::Value>>(PACKED, &["overflow"]);
    assert_canonical_absence::<Refusal>(REFUSAL, &["witness", "expected", "observed"]);
}

/// Every crossing optional field carries the whole spelling, including the ones
/// no document above names.
#[test]
fn every_crossing_optional_field_spells_absence_one_way() {
    let findings = judge_tree(&crate_root().join("src"));

    assert!(
        findings.is_empty(),
        "optional fields admitting a second spelling of absence:\n{}",
        findings.join("\n")
    );
}

/// The sweep above is enforcement, not an observation that today's crate
/// complies.
///
/// Without this fixture, emptying the source collector, stopping the walk at
/// the top directory, dropping the descent into modules or enum variants,
/// skipping the fields that carry no ident, or short-circuiting the judgment
/// all leave the suite green while nothing is enforced. The fixture tree
/// carries one offender at each of those depths, in both field syntaxes, and
/// three declarations that must stay unreported, so the finding list is exact
/// in both directions.
#[test]
fn an_optional_field_missing_the_spelling_is_refused() {
    let findings = judge_tree(&crate_root().join("tests/fixture/absence"));

    assert_eq!(
        findings,
        vec![
            format!("bare.rs: Draft.note omits {DEFAULT}, {SKIP}, {PRESENT}"),
            format!("bare.rs: Step::Begun.after omits {DEFAULT}, {SKIP}, {PRESENT}"),
            format!("nested/partial.rs: Partial.cause omits {PRESENT}"),
            format!("unnamed.rs: Tagged.1 omits {DEFAULT}, {SKIP}, {PRESENT}"),
            format!("unnamed.rs: Reach::Near.0 omits {DEFAULT}, {SKIP}, {PRESENT}"),
            format!("unnamed.rs: Reach::Far.0 omits {DEFAULT}, {PRESENT}"),
        ]
    );
}

fn assert_canonical_absence<T>(document: &str, absent_fields: &[&str])
where
    T: DeserializeOwned + Serialize,
{
    let decoded: T = wire::decode(document.as_bytes()).expect("omitted fields decode");
    let original: serde_json::Value = serde_json::from_str(document).expect("fixture is JSON");
    let encoded: serde_json::Value =
        serde_json::from_slice(&wire::encode(&decoded).expect("fixture encodes"))
            .expect("encoded fixture is JSON");
    assert_eq!(encoded, original, "absence did not remain omitted");

    for field in absent_fields {
        let mut alternate = original.clone();
        alternate
            .as_object_mut()
            .expect("fixture is an object")
            .insert((*field).to_owned(), serde_json::Value::Null);
        let bytes = serde_json::to_vec(&alternate).expect("alternate encodes as JSON");
        assert!(
            wire::decode::<T>(&bytes).is_err(),
            "{field}: null admitted a second spelling for absence"
        );
    }
}

/// Omission decodes to `None` rather than failing for a missing key.
const DEFAULT: &str = "default";

/// `None` encodes to an omitted key rather than to `null`.
const SKIP: &str = r#"skip_serializing_if = "Option::is_none""#;

/// A present key must carry a `T`, so `null` is refused instead of becoming a
/// second spelling of absence.
const PRESENT: &str = r#"deserialize_with = "crate::optional::present""#;

/// Which halves of the crossing a declaration performs, and so which of the
/// three attributes it must carry.
#[derive(Clone, Copy)]
struct Crossing {
    serializes: bool,
    deserializes: bool,
}

/// The spelling one optional field actually declares.
#[derive(Default)]
struct Spelling {
    default: bool,
    skip: bool,
    present: bool,
}

/// One file's findings, in declaration order.
struct Judge {
    location: String,
    findings: Vec<String>,
}

impl Crossing {
    /// The derives on an item, or `None` when it never crosses.
    fn read(attributes: &[syn::Attribute]) -> Option<Self> {
        let mut crossing = Self {
            serializes: false,
            deserializes: false,
        };
        for attribute in attributes {
            if !attribute.path().is_ident("derive") {
                continue;
            }
            let Ok(entries) =
                attribute.parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
            else {
                continue;
            };
            for entry in entries {
                let Some(last) = entry.segments.last() else {
                    continue;
                };
                if last.ident == "Serialize" {
                    crossing.serializes = true;
                } else if last.ident == "Deserialize" {
                    crossing.deserializes = true;
                }
            }
        }
        (crossing.serializes || crossing.deserializes).then_some(crossing)
    }
}

impl Spelling {
    fn read(attributes: &[syn::Attribute]) -> Self {
        let mut spelling = Self::default();
        for attribute in attributes {
            if !attribute.path().is_ident("serde") {
                continue;
            }
            let Ok(entries) =
                attribute.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            else {
                continue;
            };
            for entry in entries {
                match entry {
                    Meta::Path(path) if path.is_ident("default") => spelling.default = true,
                    Meta::NameValue(pair) => {
                        let Some(value) = string_literal(&pair.value) else {
                            continue;
                        };
                        if pair.path.is_ident("skip_serializing_if") && value == "Option::is_none" {
                            spelling.skip = true;
                        } else if pair.path.is_ident("deserialize_with")
                            && value == "crate::optional::present"
                        {
                            spelling.present = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        spelling
    }

    /// The attributes this field owes, in the order the rule states them.
    fn missing(&self, crossing: Crossing) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if crossing.deserializes && !self.default {
            missing.push(DEFAULT);
        }
        if crossing.serializes && !self.skip {
            missing.push(SKIP);
        }
        if crossing.deserializes && !self.present {
            missing.push(PRESENT);
        }
        missing
    }
}

impl Judge {
    fn new(location: &str) -> Self {
        Self {
            location: location.to_owned(),
            findings: Vec::new(),
        }
    }

    fn items(&mut self, items: &[syn::Item]) {
        for item in items {
            match item {
                syn::Item::Struct(node) => {
                    if let Some(crossing) = Crossing::read(&node.attrs) {
                        self.fields(&node.ident.to_string(), crossing, &node.fields);
                    }
                }
                syn::Item::Enum(node) => {
                    if let Some(crossing) = Crossing::read(&node.attrs) {
                        for variant in &node.variants {
                            let owner = format!("{}::{}", node.ident, variant.ident);
                            self.fields(&owner, crossing, &variant.fields);
                        }
                    }
                }
                syn::Item::Mod(node) => {
                    if let Some((_, nested)) = &node.content {
                        self.items(nested);
                    }
                }
                _ => {}
            }
        }
    }

    /// Judge every optional field a declaration carries, named the way Rust
    /// names it.
    ///
    /// A tuple field answers to its position rather than to an ident. Reading
    /// the ident and skipping the fields that have none would exempt every
    /// tuple struct and tuple variant from the rule, so absence is judged by
    /// the field's type and attributes and the position only spells it.
    fn fields(&mut self, owner: &str, crossing: Crossing, fields: &syn::Fields) {
        for (position, field) in fields.iter().enumerate() {
            if !is_optional(&field.ty) {
                continue;
            }
            let missing = Spelling::read(&field.attrs).missing(crossing);
            if missing.is_empty() {
                continue;
            }
            let name = field
                .ident
                .as_ref()
                .map_or_else(|| position.to_string(), ToString::to_string);
            self.findings.push(format!(
                "{}: {owner}.{name} omits {}",
                self.location,
                missing.join(", ")
            ));
        }
    }
}

/// An optional field is one whose own type is `Option`; a `Vec<Option<T>>`
/// carries its absence inside a present key.
fn is_optional(declared: &syn::Type) -> bool {
    let syn::Type::Path(path) = declared else {
        return false;
    };
    path.qself.is_none()
        && path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Option")
}

fn string_literal(expression: &syn::Expr) -> Option<String> {
    let syn::Expr::Lit(node) = expression else {
        return None;
    };
    let syn::Lit::Str(text) = &node.lit else {
        return None;
    };
    Some(text.value())
}

fn judge_tree(root: &Path) -> Vec<String> {
    let mut sources = Vec::new();
    collect_rust_sources(root, &mut sources);
    sources.sort();

    let mut findings = Vec::new();
    for path in &sources {
        let source = fs::read_to_string(path).expect("Rust source must be readable");
        let location = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        findings.extend(judge_source(&location, &source));
    }
    findings
}

fn judge_source(location: &str, source: &str) -> Vec<String> {
    let syntax =
        syn::parse_file(source).unwrap_or_else(|error| panic!("{location} must parse: {error}"));
    let mut judge = Judge::new(location);
    judge.items(&syntax.items);
    judge.findings
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn collect_rust_sources(directory: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("judged directory must be readable") {
        let entry = entry.expect("judged entry must be readable");
        let path = entry.path();
        let file_type = entry.file_type().expect("judged entry must have a type");
        if file_type.is_dir() {
            collect_rust_sources(&path, sources);
        } else if file_type.is_file() && path.extension().is_some_and(|value| value == "rs") {
            sources.push(path);
        }
    }
}
