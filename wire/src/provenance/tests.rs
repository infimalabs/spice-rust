use super::*;

const DIGEST: &str = "0bd1d1c24587b6b856cbd3172385e466e3f8a419c291e6a067b3cdce02867034";

#[test]
fn every_variant_round_trips_through_its_one_spelling() {
    for text in [
        "ack:dTKHYYzQy",
        "ack:1kQnvnHN",
        "task:dWB423Bgc",
        &format!("doc:{DIGEST}"),
        "operator:draft",
        "parentless",
    ] {
        let provenance: TaskProvenance = text.parse().expect("canonical provenance");
        assert_eq!(provenance.to_string(), text);
        let json = serde_json::to_string(&provenance).expect("encode");
        assert_eq!(
            serde_json::from_str::<TaskProvenance>(&json).expect("decode"),
            provenance
        );
    }
}

#[test]
fn the_strict_decoder_refuses_retired_and_unshaped_spellings() {
    for text in [
        "steering:dTKHYYzQy",
        "steer:1kQnvnHN",
        "review:CARDS-1kRYkKfc",
        "task:CARDS-1kRYkKfc",
        "gate:complexity",
        "ack:not-a-key",
        "operator:other",
        "parentless:",
    ] {
        assert!(
            serde_json::from_str::<TaskProvenance>(&format!("{text:?}")).is_err(),
            "{text} decoded"
        );
    }
}

#[test]
fn authored_origins_admit_only_steering_and_parent_tasks() {
    assert_eq!(
        TaskProvenance::authored("ack:dTKHYYzQy"),
        Ok(TaskProvenance::Steering("dTKHYYzQy".to_owned()))
    );
    let parent = TaskProvenance::Task("1kRYkKfc".parse().expect("stamp"));
    assert_eq!(
        TaskProvenance::authored("task:1kRYkKfc").as_ref(),
        Ok(&parent)
    );
    assert_eq!(
        TaskProvenance::authored("task:CARDS-1kRYkKfc").as_ref(),
        Ok(&parent),
        "a handle names its birth stamp"
    );
    for retired in [
        "steering:dTKHYYzQy",
        "steer:dTKHYYzQy",
        "directive:dTKHYYzQy",
    ] {
        let error = TaskProvenance::authored(retired).expect_err("retired steering class");
        assert_eq!(error.canonical(), Some("ack:dTKHYYzQy"));
    }
    for minted in [
        &format!("doc:{DIGEST}"),
        "operator:draft",
        "parentless",
        "gate:complexity",
        "review:CARDS-1kRYkKfc",
    ] {
        let error = TaskProvenance::authored(minted).expect_err("not authorable");
        assert_eq!(error.canonical(), None, "{minted}");
    }
}

#[test]
fn only_parentless_lacks_a_creation_origin() {
    assert_eq!(TaskProvenance::Parentless.origin(), None);
    assert_eq!(
        TaskProvenance::OperatorDraft
            .origin()
            .map(|origin| origin.to_string()),
        Some("operator:draft".to_owned())
    );
}
