use super::*;

fn sent(text: &str) -> SteerSent {
    SteerSent {
        from: None,
        directive: "00000000-0000-4000-8000-000000000001".parse().unwrap(),
        fanout: Some("00000000-0000-4000-8000-000000000004".parse().unwrap()),
        key: "taste-1".to_owned(),
        lane: "00000000-0000-4000-8000-000000000002".parse().unwrap(),
        text: text.to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: "2026-08-09T20:00:00.000Z".parse().unwrap(),
    }
}

#[test]
fn correction_filter_preserves_exact_authored_excerpt_and_excludes_noise() {
    let text = "Context.\n  Do not use grep here; use rg instead.\nThanks";
    let (excerpt, words) = authored_correction(&sent(text)).expect("correction qualifies");
    assert_eq!(excerpt, "  Do not use grep here; use rg instead.");
    assert!(words.contains("grep"));
    assert!(words.contains("rg"));
    for excluded in [
        "Maybe use rg instead.",
        "Status update: use rg.",
        "```diff\n-use grep\n+use rg\n```",
        "Tool output: do not use grep",
        "API key secret: do not use grep",
        "🌶️ last maxim: do not use grep",
    ] {
        assert!(authored_correction(&sent(excluded)).is_none(), "{excluded}");
    }
}

#[test]
fn pem_and_common_credentials_never_reach_exact_evidence() {
    for secret in [
        "Do not print -----BEGIN PRIVATE KEY----- abcdefgh.",
        "Never expose bearer abcdefghijklmnop.",
        "Avoid logging password=hunter2.",
        "Do not paste github_pat_abcdefgh12345678.",
        "Never retain AKIAABCDEFGHIJKLMNOP.",
    ] {
        assert!(super::super::contains_sensitive_text(secret), "{secret}");
        assert!(authored_correction(&sent(secret)).is_none(), "{secret}");
    }
    assert!(super::super::contains_sensitive_text(
        "declined because credential: abcdefgh was exposed"
    ));
}

#[test]
fn near_duplicate_theme_qualifies_once_with_deterministic_exact_citations() {
    let corrections = [
        "Do not use grep here; use rg instead.",
        "Prefer rg rather than grep for searches.",
        "Never reach for grep; always use rg.",
        "Always validate config with cargo metadata.",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, text)| correction(index as u64 + 1, text))
    .collect::<Vec<_>>();
    let candidates = cluster(&corrections, 3);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].corrections.len(), 3);
    assert_eq!(candidates[0].message, corrections[0].excerpt);
    let evidence = description(&candidates[0], 3, None);
    for correction in &corrections[..3] {
        assert!(evidence.contains(&format!("{:?}", correction.excerpt)));
    }
}

#[test]
fn bridge_correction_cannot_synthesize_a_theme_shared_by_no_threshold_set() {
    let corrections = vec![
        correction(1, "Use alpha beta one."),
        correction(2, "Use alpha beta gamma delta."),
        correction(3, "Use gamma delta three."),
    ];
    assert!(cluster(&corrections, 3).is_empty());
}

#[test]
fn one_operator_fanout_across_distinct_lane_tapes_counts_once() {
    let first = TranscriptLocus {
        source: Some(directive(701)),
        member: 1,
        offset: 10,
        length: 50,
        event: 0,
        line: 1,
        column: 1,
    };
    let second = TranscriptLocus {
        source: Some(directive(702)),
        member: 2,
        offset: 20,
        ..first
    };
    let distinct = TranscriptLocus {
        source: Some(directive(703)),
        member: 3,
        offset: 30,
        ..first
    };
    let evidence = [
        (
            1,
            "00000000-0000-4000-8000-000000000010",
            "Do not use grep; use rg instead.",
            first,
        ),
        (
            2,
            "00000000-0000-4000-8000-000000000020",
            "Prefer rg rather than grep.",
            second,
        ),
        (
            3,
            "00000000-0000-4000-8000-000000000010",
            "Never use grep; always use rg.",
            distinct,
        ),
    ];
    let mut envelopes = Vec::new();
    for (position, lane, text, _) in evidence {
        envelopes.push(envelope(
            position,
            Actor::Operator,
            SteerSent {
                from: None,
                directive: directive(position),
                fanout: Some(if position <= 2 {
                    directive(700)
                } else {
                    directive(703)
                }),
                key: format!("taste-{position}"),
                lane: lane.parse().unwrap(),
                text: text.to_owned(),
                control: None,
                attachments: Vec::new(),
                expires: "2026-08-09T20:00:00.000Z".parse().unwrap(),
            },
        ));
    }
    for (position, lane, _, locus) in evidence {
        envelopes.push(envelope(
            position + 3,
            Actor::Spice,
            SteerAcked {
                directive: directive(position),
                key: format!("taste-{position}"),
                lane: lane.parse().unwrap(),
                epoch: "00000000-0000-4000-8000-000000000030".parse().unwrap(),
                locus,
            },
        ));
    }
    let snapshot = journal::FactSnapshot::from_facts(envelopes).unwrap();
    let (corrections, scanned, excluded) = match corrections(&snapshot) {
        Ok(observed) => observed,
        Err(_) => panic!("typed multi-lane evidence must remain available"),
    };

    assert_eq!((scanned, excluded, corrections.len()), (3, 0, 3));
    assert!(cluster(&corrections, 3).is_empty());
}

#[test]
fn distinct_transcript_loci_still_satisfy_recurrence() {
    let corrections = vec![
        correction(1, "Do not use grep; use rg instead."),
        correction(2, "Prefer rg rather than grep."),
        correction(3, "Never use grep; always use rg."),
    ];
    assert_eq!(cluster(&corrections, 3)[0].corrections.len(), 3);
}

#[test]
fn unrelated_fanouts_at_equal_local_coordinates_count_independently() {
    let mut corrections = vec![
        correction(1, "Do not use grep; use rg instead."),
        correction(2, "Prefer rg rather than grep."),
        correction(3, "Never use grep; always use rg."),
    ];
    let coordinates = TranscriptLocus {
        source: None,
        member: 1,
        offset: 10,
        length: 50,
        event: 0,
        line: 1,
        column: 1,
    };
    for (index, correction) in corrections.iter_mut().enumerate() {
        correction.locus = TranscriptLocus {
            source: Some(directive(800 + u64::try_from(index).unwrap())),
            ..coordinates
        };
    }

    let candidates = cluster(&corrections, 3);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].corrections.len(), 3);
}

#[test]
fn completed_predecessor_refiles_only_after_genuine_new_recurrence() {
    let base = vec![
        correction(1, "Do not use grep; use rg instead."),
        correction(2, "Prefer rg rather than grep."),
        correction(3, "Never use grep; always use rg."),
    ];
    let theme = ("grep".to_owned(), "rg".to_owned());
    let first = candidate(base.clone(), theme.clone());
    let prior = PriorProposal {
        handle: "TASTE-1kKRRsDP".parse().unwrap(),
        occurrence: first.occurrence.clone(),
        recurrence: 3,
        completed: true,
    };
    let priors = BTreeMap::from([(first.finding.clone(), vec![prior.clone()])]);
    let (standing, _, dependency) = plan(&first, &priors);
    assert_eq!(standing, ProposalStanding::ReusedCompleted);
    assert!(dependency.is_none());

    let mut expanded = base;
    expanded.push(correction(4, "Avoid grep and use rg for searches."));
    let later = candidate(expanded, theme);
    let (standing, _, dependency) = plan(&later, &priors);
    assert_eq!(standing, ProposalStanding::Filed);
    assert_eq!(dependency, Some(prior.handle));
}

#[test]
fn unavailable_ack_context_is_a_loud_store_fault() {
    let directive = "00000000-0000-4000-8000-000000000099".parse().unwrap();
    let fact = wire::Fact::new(SteerAcked {
        directive,
        key: "missing-ack".to_owned(),
        lane: "00000000-0000-4000-8000-000000000002".parse().unwrap(),
        epoch: "00000000-0000-4000-8000-000000000003".parse().unwrap(),
        locus: TranscriptLocus {
            source: Some("00000000-0000-4000-8000-000000000004".parse().unwrap()),
            member: 1,
            offset: 10,
            length: 50,
            event: 0,
            line: 1,
            column: 1,
        },
    });
    let opaque = wire::decode::<wire::AnyFact>(&wire::encode(&fact).unwrap()).unwrap();
    let snapshot = journal::FactSnapshot::from_facts(vec![wire::Envelope {
        v: wire::WireVersion::CURRENT,
        seq: Seq::new(1).unwrap(),
        at: "2026-08-09T20:00:00.000Z".parse().unwrap(),
        actor: Actor::Spice,
        origin: "taste:test".parse().unwrap(),
        cause: None,
        fact: opaque,
    }])
    .unwrap();
    assert!(matches!(
        corrections(&snapshot),
        Err(EvidenceError::AnswerWithoutSent(missing)) if missing == directive
    ));
    let Outcome::Fault(fault) = evidence_fault(EvidenceError::AnswerWithoutSent(directive)) else {
        panic!("unavailable evidence must fault")
    };
    assert_eq!(fault.reason, "taste::mining-evidence-invalid");
}

fn correction(position: u64, text: &str) -> Correction {
    Correction {
        seq: Seq::new(position).unwrap(),
        directive: directive(position),
        fanout: Some(directive(position + 300)),
        key: format!("taste-{position}"),
        excerpt: text.to_owned(),
        tokens: tokens(text),
        lane: directive(position + 100),
        locus: TranscriptLocus {
            source: Some(directive(position + 200)),
            member: position,
            offset: position * 10,
            length: 50,
            event: 0,
            line: 1,
            column: 1,
        },
        disposition: "acked",
        decline_reason: None,
    }
}

fn directive(position: u64) -> CanonicalUuid {
    format!("00000000-0000-4000-8000-{position:012}")
        .parse()
        .unwrap()
}

fn envelope<F: wire::Body>(position: u64, actor: Actor, body: F) -> wire::Envelope<wire::AnyFact> {
    let fact = wire::Fact::new(body);
    let opaque = wire::decode::<wire::AnyFact>(&wire::encode(&fact).unwrap()).unwrap();
    wire::Envelope {
        v: wire::WireVersion::CURRENT,
        seq: Seq::new(position).unwrap(),
        at: "2026-08-09T20:00:00.000Z".parse().unwrap(),
        actor,
        origin: "taste:test".parse().unwrap(),
        cause: None,
        fact: opaque,
    }
}
