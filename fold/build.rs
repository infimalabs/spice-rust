#[path = "build/admission.rs"]
mod admission;
fn main() -> Result<(), admission::Refusal> {
    println!("cargo::rerun-if-changed=build/admission.rs");
    println!("cargo::rerun-if-changed=build/registry.rs");
    println!("cargo::rerun-if-changed=src/registry.rs");

    admission::validate_registry_source(wire::FactKind::ALL, include_str!("src/registry.rs"))
}
