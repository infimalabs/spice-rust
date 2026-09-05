#[path = "build/admission.rs"]
mod admission;
#[path = "build/registry.rs"]
mod registry;

fn main() -> Result<(), admission::Refusal> {
    println!("cargo::rerun-if-changed=build/admission.rs");
    println!("cargo::rerun-if-changed=build/registry.rs");
    println!("cargo::rerun-if-changed=src/registry.rs");

    admission::validate(wire::FactKind::ALL, registry::FOLDS)
}
