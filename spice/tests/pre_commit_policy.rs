use spice::config::{Inputs, resolve};

#[test]
fn generic_rust_policy_without_the_source_override_keeps_packaged_builtins() {
    let generic = resolve(Inputs::new(None, None, &[]))
        .expect("an absent tracked document resolves to packaged defaults");

    assert!(generic.root().policy().pre_commit_builtins());
    assert!(generic.root().policy().pre_commit().is_empty());
}
