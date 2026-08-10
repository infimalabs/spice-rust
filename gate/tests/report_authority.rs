//! Report decoding has an observation-only type graph.
//!
//! This test reads the declarations the decoder actually builds. It refuses
//! any executable authority in a retained field or any public report method
//! that could return such authority after decoding.

use std::fs;
use std::path::{Path, PathBuf};

use syn::{Fields, GenericArgument, ImplItem, Item, PathArguments, ReturnType, Type, Visibility};

const FORBIDDEN: &[&str] = &["Contract", "Camp", "Kill", "Argv", "Bounds", "KillBounds"];
const REPORT_SOURCES: &[&str] = &[
    "src/report.rs",
    "src/report/gate.rs",
    "src/report/kill.rs",
    "src/report/render.rs",
    "src/camp.rs",
    "src/ratchet.rs",
    "src/test_evidence.rs",
    "src/verdict.rs",
];

#[test]
fn decoded_reports_expose_no_executable_reconstruction_authority() {
    let mut retained = Vec::new();
    let mut violations = Vec::new();

    for relative in REPORT_SOURCES {
        let source = fs::read_to_string(crate_root().join(relative))
            .unwrap_or_else(|error| panic!("read {relative}: {error}"));
        let file = syn::parse_file(&source)
            .unwrap_or_else(|error| panic!("parse {relative} as Rust: {error}"));
        scan_file(relative, &file, &mut retained, &mut violations);
    }

    for required in [
        "GateReport",
        "KillReport",
        "CampReport",
        "SuiteSeamReport",
        "MutationCoverageReport",
        "TestSubsumptionReport",
        "TestEvidenceReport",
        "Verdict",
    ] {
        assert!(
            retained.iter().any(|name| name == required),
            "authority scan did not inspect {required}"
        );
    }
    assert!(
        violations.is_empty(),
        "decoded report graph exposes executable authority:\n{}",
        violations.join("\n")
    );
}

fn scan_file(
    relative: &str,
    file: &syn::File,
    retained: &mut Vec<String>,
    violations: &mut Vec<String>,
) {
    for item in &file.items {
        match item {
            Item::Struct(item) if is_retained(&item.ident.to_string()) => {
                let name = item.ident.to_string();
                retained.push(name.clone());
                scan_fields(relative, &name, &item.fields, violations);
            }
            Item::Enum(item) if is_retained(&item.ident.to_string()) => {
                let name = item.ident.to_string();
                retained.push(name.clone());
                for variant in &item.variants {
                    scan_fields(
                        relative,
                        &format!("{name}::{}", variant.ident),
                        &variant.fields,
                        violations,
                    );
                }
            }
            Item::Impl(item) if retained_self_type(&item.self_ty).is_some() => {
                let name = retained_self_type(&item.self_ty).expect("checked report type");
                for member in &item.items {
                    if let ImplItem::Fn(method) = member
                        && matches!(method.vis, Visibility::Public(_))
                        && let ReturnType::Type(_, output) = &method.sig.output
                    {
                        record_forbidden(
                            relative,
                            &format!("{name}::{} return type", method.sig.ident),
                            output,
                            violations,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn scan_fields(relative: &str, owner: &str, fields: &Fields, violations: &mut Vec<String>) {
    for (index, field) in fields.iter().enumerate() {
        let name = field
            .ident
            .as_ref()
            .map_or_else(|| index.to_string(), ToString::to_string);
        record_forbidden(relative, &format!("{owner}.{name}"), &field.ty, violations);
    }
}

fn record_forbidden(relative: &str, location: &str, ty: &Type, violations: &mut Vec<String>) {
    let mut found = Vec::new();
    forbidden_types(ty, &mut found);
    for forbidden in found {
        violations.push(format!("{relative}: {location} contains {forbidden}"));
    }
}

fn forbidden_types<'a>(ty: &'a Type, found: &mut Vec<&'a str>) {
    match ty {
        Type::Array(array) => forbidden_types(&array.elem, found),
        Type::Group(group) => forbidden_types(&group.elem, found),
        Type::Paren(paren) => forbidden_types(&paren.elem, found),
        Type::Path(path) => {
            for segment in &path.path.segments {
                let name = segment.ident.to_string();
                if FORBIDDEN.contains(&name.as_str()) {
                    found.push(
                        FORBIDDEN
                            .iter()
                            .find(|value| **value == name)
                            .expect("found"),
                    );
                }
                if let PathArguments::AngleBracketed(arguments) = &segment.arguments {
                    for argument in &arguments.args {
                        if let GenericArgument::Type(ty) = argument {
                            forbidden_types(ty, found);
                        }
                    }
                }
            }
        }
        Type::Ptr(pointer) => forbidden_types(&pointer.elem, found),
        Type::Reference(reference) => forbidden_types(&reference.elem, found),
        Type::Slice(slice) => forbidden_types(&slice.elem, found),
        Type::Tuple(tuple) => {
            for element in &tuple.elems {
                forbidden_types(element, found);
            }
        }
        _ => {}
    }
}

fn retained_self_type(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    let name = path.path.segments.last()?.ident.to_string();
    is_retained(&name).then_some(name)
}

fn is_retained(name: &str) -> bool {
    name == "Verdict"
        || name == "ReceiptTool"
        || name == "Standing"
        || name == "Finding"
        || name == "Movement"
        || name == "DistributionReport"
        || name == "CampFinding"
        || name == "CampStanding"
        || name == "BoundRelation"
        || name == "LocusDistance"
        || name.ends_with("Report")
        || name.starts_with("Report")
}

fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}
