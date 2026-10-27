use super::*;

/// Every example a refusal shows decodes as its own command's body, and a
/// malformed body of that operand shows exactly that example.
fn assert_operand_teaches_its_shape<T: DeserializeOwned>(operand: &JsonOperand<T>) {
    assert!(
        operand.decode(operand.example).is_ok(),
        "{} example refuses: {}",
        operand.name,
        operand.example
    );
    let Err(Outcome::Refusal(refusal)) = operand.decode("{}") else {
        panic!("{} accepted an empty body", operand.name);
    };
    let diagnostic = refusal.diagnostic().expect("a refusal diagnostic");
    assert_eq!(diagnostic.reason(), "preparation::argument-invalid");
    let detail = diagnostic.detail().expect("a refusal detail");
    assert!(
        detail.contains(operand.example),
        "{} refusal omits its example: {detail}",
        operand.name
    );
}

#[test]
fn each_json_operand_refusal_shows_a_body_its_command_accepts() {
    assert_operand_teaches_its_shape(&DECISION_JSON);
    assert_operand_teaches_its_shape(&WITHDRAWAL_JSON);
}
