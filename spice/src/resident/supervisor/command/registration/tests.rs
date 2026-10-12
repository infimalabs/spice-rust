use std::sync::Arc;

use super::Driver;

/// A first turn as a launch composes it: a cut row naming a read a resource
/// serves, and a placeholder no resource can.
const PROMPT: &str = "# Spice briefing\n- … 3 more: `spice session sweep /t/x.jsonl --limit 4`\n- held: `spice task show HANDLE`";
const OPENING: &[u8] = b"{\"id\":\"spice-state\",\"type\":\"get_state\"}\n";

fn driver() -> Arc<Driver<Vec<u8>>> {
    Driver::with_readers(
        Vec::new(),
        || None,
        || None,
        |_| Err("unread".to_owned()),
        steer::Key::for_identity(crate::resident::new_uuid()),
        None,
    )
}

fn frames(driver: &Driver<Vec<u8>>) -> Vec<serde_json::Value> {
    let input = driver.stdin.lock().expect("input locks");
    input
        .as_ref()
        .expect("input remains open")
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("a frame"))
        .collect()
}

fn answer(driver: &Arc<Driver<Vec<u8>>>, line: &[u8]) {
    let locus = listen::Locus::new(0, 0, line.len() as u64);
    let decoded = listen::decode(listen::Dialect::Omp, line, locus).expect("locus spans the line");
    let _ask = driver.observe(&decoded);
}

fn registration(success: bool) -> Vec<u8> {
    let mut answer = serde_json::json!({
        "id": listen::OMP_URI_SCHEMES_REQUEST_ID,
        "type": "response",
        "command": "set_host_uri_schemes",
        "success": success,
    });
    if !success {
        answer["error"] = "Unknown command: set_host_uri_schemes".into();
    }
    serde_json::to_vec(&answer).expect("JSON")
}

#[test]
fn only_a_seat_whose_registration_succeeded_is_told_the_scheme_and_handed_urls() {
    for success in [true, false] {
        let driver = driver();
        driver
            .open(OPENING, Some(PROMPT.to_owned()))
            .expect("opening writes");
        answer(
            &driver,
            br#"{"id":"spice-effort","type":"response","command":"set_thinking_level","success":true}"#,
        );
        assert_eq!(
            frames(&driver).len(),
            1,
            "the first turn waits for the registration answer"
        );

        answer(&driver, &registration(success));
        answer(&driver, &registration(!success));
        let frames = frames(&driver);
        assert_eq!(
            frames.len(),
            2,
            "one first turn, settled by the first answer"
        );
        assert_eq!(frames[1]["id"], vendor::omp::PROMPT_REQUEST_ID);
        assert_eq!(
            frames[1]["streamingBehavior"], "followUp",
            "a restored in-flight turn queues this one first prompt"
        );
        let message = frames[1]["message"].as_str().expect("a prompt message");
        if success {
            assert!(message.contains("# spice:// reads"), "{message}");
            assert!(message.contains("`spice://session/sweep?"), "{message}");
            assert!(!message.contains("`spice session sweep"), "{message}");
            assert!(message.contains("`spice task show HANDLE`"), "{message}");
        } else {
            assert_eq!(message, PROMPT);
        }
    }
}
