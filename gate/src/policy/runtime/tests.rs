use super::scaled;

/// The first binary64 integer outside `u64` is also how `u64::MAX` converts to
/// binary64. A strict comparison would therefore admit it and let Rust's float
/// cast saturate, silently replacing a refused calculation with a different
/// ceiling.
#[test]
fn the_first_unrepresentable_scaled_integer_refuses_instead_of_saturating() {
    let base = 1_u64 << 63;
    let error = scaled(base, 2.0, "file.lines")
        .expect_err("two to the sixty-fourth does not fit in an unsigned 64-bit ceiling");

    assert_eq!(error.code(), "policy::calculation-refused");
    assert_eq!(error.metric(), "file.lines");
    assert!(
        error.to_string().contains("exceeds u64"),
        "the refusal must name the representability boundary: {error}"
    );
}
