#[path = "build/admission.rs"]
mod admission;
#[path = "build/registry.rs"]
mod registry;

fn main() {
    println!("cargo::rerun-if-changed=build/admission.rs");
    println!("cargo::rerun-if-changed=build/registry.rs");
    println!("cargo::rerun-if-changed=src/registry.rs");

    admission::validate(wire::FactKind::ALL, registry::FOLDS).unwrap_or_else(|refusal| {
        panic!("generated fold registry refused: {refusal}");
    });
}
