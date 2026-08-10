use wire::body::TranscriptLocus;

#[test]
fn legacy_locus_remains_byte_canonical_without_a_source() {
    let bytes = br#"{"member":1,"offset":2,"length":3,"event":4,"line":5,"column":6}"#;
    let locus = wire::decode::<TranscriptLocus>(bytes).expect("legacy locus decodes");
    assert_eq!(locus.source, None);
    assert_eq!(
        wire::encode(&locus).expect("legacy locus re-encodes"),
        bytes
    );
}

#[test]
fn sourced_locus_round_trips_its_tape_generation() {
    let locus = TranscriptLocus {
        source: Some(
            "018f82d4-6f59-7d06-92e3-bce913116399"
                .parse()
                .expect("canonical source"),
        ),
        member: 1,
        offset: 2,
        length: 3,
        event: 4,
        line: 5,
        column: 6,
    };
    let bytes = wire::encode(&locus).expect("sourced locus encodes");
    assert_eq!(
        wire::decode::<TranscriptLocus>(&bytes).expect("sourced locus decodes"),
        locus
    );
}
