use std::path::{Path, PathBuf};

use gate::{
    ArmFinding, ArmOutcome, Contract, CsharpUnusedStatus, FindingDetail, GateId, RimCapture,
    Selector, ToolchainSource, acquire_toolchain, measure_toolchain,
};

mod support;

use support::{contract_repository, git_commit};

/// A nested member answers to the innermost type that declares it.
///
/// The outer class is partial and would retain an unreached member, while the
/// inner record is complete in this file and therefore does not. This contrast
/// makes the enclosing-type choice observable: choosing the outermost span
/// would silently retain `Loose` instead of reporting it.
#[test]
fn csharp_unused_binds_a_nested_member_to_its_innermost_enclosing_type() {
    let repository = contract_repository("toolchain-csharp-nested-record");
    repository.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n",
    );
    repository.write("src/main.rs", "fn main() {}\n");
    repository.write("docs/guide.md", "Every sentence here is ordinary.\n");
    repository.write("web/app.js", "void 0;\n");
    repository.write(
        "dotnet/Nested.cs",
        concat!(
            "public partial class Outer\n{\n",
            "    private record Inner\n    {\n",
            "        private void Loose() { }\n    }\n}\n",
        ),
    );
    git_commit(&repository, "csharp nested record fixture");

    let tree = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the captured index");
    let contract = Contract::load(tree.root()).expect("load the captured contract");
    let source = ToolchainSource::capture(
        &tree,
        RimCapture::new(
            &repository.root,
            Some(toolchain_directory().into_os_string()),
            Vec::new(),
        )
        .expect("the fixture repository root is absolute"),
    );
    let observations = acquire_toolchain(&source, &contract);
    let observation = observations
        .iter()
        .find(|observation| observation.gate_id == GateId::CsharpUnusedCandidates)
        .expect("the C# arm was observed");

    assert_eq!(observation.outcome, ArmOutcome::Findings);
    assert_eq!(
        observation.findings,
        [ArmFinding {
            path: Some("dotnet/Nested.cs".to_owned()),
            line: Some(5),
            column: None,
            detail: FindingDetail::CsharpUnused {
                symbol: "Loose".to_owned(),
                declaration_kind: "method".to_owned(),
                status: CsharpUnusedStatus::CandidateUnused,
                reason: "no_references_outside_declaration".to_owned(),
                reference_count: 1,
            },
        }],
        "the inner record's unreached private member was not accused",
    );

    measure_toolchain(observations).expect("nested C# findings are admitted");
}

/// The directory holding the Cargo that built this test.
fn toolchain_directory() -> PathBuf {
    Path::new(env!("CARGO"))
        .parent()
        .expect("the building Cargo sits in a directory")
        .to_owned()
}
