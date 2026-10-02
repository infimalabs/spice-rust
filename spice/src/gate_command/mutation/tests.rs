use super::*;
use gate::SurvivorRate;
use wire::{Digest, Domain};

const INTERVAL: Duration = Duration::from_secs(30);
const MUTANTS: usize = 500;
const STEP: Duration = Duration::from_secs(2);
/// Lines a 1000-second campaign may write: one heartbeat per interval plus
/// the campaign line, ten named findings, and one omission line.
const LINE_BUDGET: usize = 60;
const CAMP_MEASUREMENTS: usize = 100;
const CAMP_DISTRIBUTIONS: usize = 4;
const CAMP_LOCI: usize = 16;
const CAMP_LINE_BUDGET: usize = 10;

fn report_rate(numerator: u64, denominator: u64) -> ReportRate {
    SurvivorRate::new(numerator, denominator)
        .expect("fixture rate")
        .into()
}

fn retained() -> Result<Export, String> {
    Ok(Export {
        path: "/captures/report.log".to_owned(),
        bytes: 462,
        digest: Digest::compute(Domain::CliExport, b"report"),
    })
}

fn identities(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("gate/src/kill.rs:{index}: replace classify -> bool with true"))
        .collect()
}

/// Lines a 500-mutant campaign writes, with the instant each was written.
/// Every mutant takes two seconds; the heartbeat thread ticks each second.
#[test]
fn a_long_campaign_names_itself_beats_only_when_quiet_and_stays_bounded() {
    let start = Instant::now();
    let mut campaign = Campaign::new(INTERVAL, start);
    let mut lines = Vec::new();
    let mut now = start;
    let tick = |campaign: &mut Campaign, now: Instant, lines: &mut Vec<(Instant, String)>| {
        if let Some(line) = campaign.heartbeat(now) {
            lines.push((now, line));
        }
    };
    // Baseline and inventory take a quiet minute before the first observation.
    for _ in 0..60 {
        now += TICK;
        tick(&mut campaign, now, &mut lines);
    }
    let names = identities(MUTANTS);
    let base = KillProgress {
        mutant: None,
        completed: 0,
        total: MUTANTS,
        attempts: 3,
        seed: 7,
        disposition: None,
    };
    lines.extend(campaign.observe(base, now).map(|line| (now, line)));
    for (index, name) in names.iter().enumerate() {
        let started = KillProgress {
            mutant: Some(name),
            completed: index,
            ..base
        };
        lines.extend(campaign.observe(started, now).map(|line| (now, line)));
        for _ in 0..STEP.as_secs() {
            now += TICK;
            tick(&mut campaign, now, &mut lines);
        }
        let disposition = if (100..131).contains(&index) {
            MutantDisposition::ConfirmedSurvived
        } else {
            MutantDisposition::ConfirmedKilled
        };
        let ended = KillProgress {
            mutant: Some(name),
            completed: index + 1,
            disposition: Some(disposition),
            ..base
        };
        lines.extend(campaign.observe(ended, now).map(|line| (now, line)));
    }

    let text = lines
        .iter()
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        text[..2],
        [
            "baseline and mutant inventory running 30.0s",
            "baseline and mutant inventory running 1m00s"
        ]
    );
    assert_eq!(text[2], "campaign seed 7: 500 mutants x 3 attempts");
    let named = text
        .iter()
        .filter(|line| line.starts_with("survived "))
        .collect::<Vec<_>>();
    assert_eq!(
        named.len(),
        LIVE_FINDINGS,
        "only the first survivors are named"
    );
    assert_eq!(
        *named[0],
        format!("survived 101/500 {}", names[100]).as_str()
    );
    assert_eq!(
        text.iter()
            .filter(|line| line.starts_with("further survivors"))
            .count(),
        1
    );
    // Quiet heartbeats carry done/total, the running mutant, and elapsed:
    // the campaign line lands at one minute, mutant 14 starts at 88s, and
    // the next heartbeat is due a full interval later.
    assert_eq!(text[3], format!("14/500 done; {} running 1m30s", names[14]));
    // Cadence: no line within an interval of the last, and no silence of
    // more than an interval and one tick, however long the campaign runs.
    for pair in lines.windows(2) {
        let gap = pair[1].0.duration_since(pair[0].0);
        assert!(
            gap <= INTERVAL + TICK,
            "silent for {gap:?} before {}",
            pair[1].1
        );
        if !pair[1].1.contains(" done; ") && !pair[1].1.contains("running") {
            continue;
        }
        assert!(gap >= INTERVAL, "heartbeat {gap:?} after {}", pair[0].1);
    }
    assert!(
        lines.len() < LINE_BUDGET,
        "{} lines for {MUTANTS} mutants",
        lines.len()
    );
}

#[test]
fn a_long_camp_campaign_names_its_shape_and_beats_with_measurement_progress() {
    let start = Instant::now();
    let mut campaign = CampCampaign::new(INTERVAL, start);
    let base = CampProgress {
        metric: None,
        completed: 0,
        total: CAMP_MEASUREMENTS,
        distributions: CAMP_DISTRIBUTIONS,
        loci: CAMP_LOCI,
    };
    let mut lines = vec![(
        start,
        campaign
            .observe(base, start)
            .expect("the initial observation names the campaign"),
    )];
    let mut now = start;
    let metrics = (0..base.total)
        .map(|index| format!("rust.metric-{index}"))
        .collect::<Vec<_>>();
    for (index, metric) in metrics.iter().enumerate() {
        campaign.observe(
            CampProgress {
                metric: Some(metric),
                completed: index,
                ..base
            },
            now,
        );
        for _ in 0..STEP.as_secs() {
            now += TICK;
            lines.extend(campaign.heartbeat(now).map(|line| (now, line)));
        }
        campaign.observe(
            CampProgress {
                metric: Some(metric),
                completed: index + 1,
                ..base
            },
            now,
        );
    }

    assert_eq!(
        lines[0].1,
        "campaign: 4 distributions, 16 loci, 100 measurements"
    );
    assert_eq!(lines[1].1, "14/100 measured; rust.metric-14 running 30.0s");
    for pair in lines.windows(2) {
        assert_eq!(
            pair[1].0.duration_since(pair[0].0),
            INTERVAL,
            "heartbeat cadence moved at {}",
            pair[1].1
        );
    }
    assert!(
        lines.len() < CAMP_LINE_BUDGET,
        "{} lines for 100 measurements",
        lines.len()
    );
}

fn outcome(
    names: &[String],
    survivors: usize,
    unconfirmed: usize,
    movement: ReportMutationMovement,
) -> KillOutcome<'_> {
    KillOutcome {
        seed: 7,
        attempts: 2,
        movement,
        rate: (unconfirmed == 0).then(|| report_rate(survivors as u64, names.len() as u64)),
        proposed: None,
        mutants: names
            .iter()
            .enumerate()
            .map(|(index, identity)| {
                let (disposition, attempts) = if index < survivors {
                    (
                        ReportMutantDisposition::ConfirmedSurvived,
                        vec![ReportAttemptKind::Survived; 2],
                    )
                } else if index < survivors + unconfirmed {
                    (
                        ReportMutantDisposition::Unconfirmed,
                        vec![ReportAttemptKind::Killed, ReportAttemptKind::TimedOut],
                    )
                } else {
                    (
                        ReportMutantDisposition::ConfirmedKilled,
                        vec![ReportAttemptKind::Killed; 2],
                    )
                };
                MutantLine {
                    identity,
                    disposition,
                    attempts,
                }
            })
            .collect(),
    }
}

#[test]
fn a_regression_lists_bounded_survivors_with_omission_and_the_next_command() {
    let names = identities(300);
    let regressed = outcome(
        &names,
        45,
        0,
        ReportMutationMovement::Regressed {
            floor: report_rate(0, 1),
            measured: report_rate(45, 300),
        },
    );
    let text = kill_summary(&regressed, Duration::from_secs(600), &retained(), false);
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(
        lines[0],
        "gate kill: regress (seed 7, 300 mutants x 2 attempts)"
    );
    assert!(
        lines[1].contains("survivor rate 3/20 exceeds floor 0/1"),
        "{text}"
    );
    let listed = lines
        .iter()
        .filter(|line| line.trim_start().starts_with("gate/src/kill.rs:"))
        .count();
    assert_eq!(listed, SHOWN_MUTANTS);
    assert!(
        lines.contains(&"          25 of 45 withheld; see spice gate kill --verbose"),
        "{text}"
    );
    assert_eq!(
        lines[lines.len() - 2..],
        [
            "report: /captures/report.log (462 bytes)",
            "next: kill each listed survivor with a test, git add, then spice gate kill"
        ]
    );

    let verbose = kill_summary(&regressed, Duration::from_secs(600), &retained(), true);
    let attempts = verbose
        .lines()
        .filter(|line| line.ends_with("attempts survived,survived"))
        .count();
    assert_eq!(
        attempts, 45,
        "--verbose names every survivor with its attempts"
    );
    assert!(!verbose.contains("withheld"), "{verbose}");
}

#[test]
fn unconfirmed_attempts_are_named_even_without_verbose() {
    let names = identities(10);
    let unsure = outcome(
        &names,
        0,
        2,
        ReportMutationMovement::Unconfirmed {
            confirmed: 8,
            total: 10,
        },
    );
    let text = kill_summary(&unsure, Duration::from_secs(5), &retained(), false);
    assert!(text.starts_with("gate kill: unsure"), "{text}");
    assert!(
        text.contains(&format!("{}  attempts killed,timed-out", names[1])),
        "{text}"
    );
    assert!(
        text.ends_with("next: make the listed attempts repeatable, git add, then spice gate kill"),
        "{text}"
    );
}

#[test]
fn a_clean_hold_reports_counts_and_commits() {
    let names = identities(50);
    let held = outcome(
        &names,
        0,
        0,
        ReportMutationMovement::Held {
            floor: report_rate(0, 1),
        },
    );
    let text = kill_summary(&held, Duration::from_secs(125), &retained(), false);
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        [
            "gate kill: held (seed 7, 50 mutants x 2 attempts)",
            "  held    survivor rate 0/1 holds floor 0/1  2m05s",
            "  killed  50 mutants confirmed killed",
            "  survived 0 confirmed survivors",
            "  unsure  0 unconfirmed mutants",
            "report: /captures/report.log (462 bytes)",
            "next: git commit",
        ]
    );
}
