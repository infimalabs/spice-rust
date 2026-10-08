//! Stored lane fast-mode facts retain explicit false and absent fields.

#[test]
fn lane_fast_facts_preserve_every_stored_flag_presence() {
    for from in [None, Some(false), Some(true)] {
        for fast_mode in [None, Some(false), Some(true)] {
            let mut supplied = serde_json::json!({
                "kind": "lane::fast",
                "lane": "018f82d4-6f59-7d06-92e3-bce913116302",
            });
            if let Some(from) = from {
                supplied["from"] = from.into();
            }
            if let Some(fast_mode) = fast_mode {
                supplied["fast_mode"] = fast_mode.into();
            }
            let encoded = serde_json::to_vec(&supplied).expect("stored fact encodes");
            let fact: wire::AnyFact = wire::decode(&encoded).expect("stored fact remains readable");
            assert_eq!(
                serde_json::to_value(&fact).expect("registered fact re-encodes"),
                supplied,
                "stored flag presence changed: from={from:?}, fast_mode={fast_mode:?}",
            );
        }
    }
}
