//! Typed quantities; figures and prose consume these records, never separate counts.

use std::collections::BTreeMap;

use super::Result;
use super::episodes::Outcomes;
use super::publication::{
    FOLLOW_UP_MS, MATERIALIZATION_BOUNDARY, MINUTE_MS, POLICY_BOUNDARY, Quantity, day, domain,
    phase, quantity,
};
use super::reduce::Study;

const MEDIAN: f64 = 0.5;
const P90: f64 = 0.9;
const P95: f64 = 0.95;
const MATERIALIZATION_SECONDS: [f64; 3] = [43.25, 0.33, 33.66];
const MATERIALIZATION_CRATES: [usize; 3] = [20, 0, 4];
const MATERIALIZATION_LABELS: [&str; 3] = [
    "Fresh materialization",
    "Unchanged refill",
    "Three-file changed refill",
];

pub(super) fn regime(position: u64) -> &'static str {
    if position < POLICY_BOUNDARY {
        "Before blocking-policy integration"
    } else if position < MATERIALIZATION_BOUNDARY {
        "After blocking-policy integration"
    } else {
        "After materialization integration"
    }
}

pub(super) fn measure(study: &Study, outcomes: &Outcomes) -> Result<Vec<Quantity>> {
    let completed = study
        .tasks
        .values()
        .filter(|task| !task.deleted && task.closure.is_some())
        .count();
    let mut result = vec![
        quantity(
            "Q01",
            "Delivery selection",
            "All admitted public task births completed at the frozen horizon",
            "tasks",
            "The most recent distinct completed births, ordered by their current completed episode and canonical birth tie-breaker. Deleted tasks are excluded.",
            completed,
            [
                ("Selected".to_owned(), study.selected.len() as f64),
                (
                    "Earlier completions outside selection".to_owned(),
                    (completed - study.selected.len()) as f64,
                ),
            ],
        ),
        quantity(
            "Q02",
            "Closure routes",
            "Selected deliveries",
            "tasks",
            "Observed closing kind and last phase; a phase done is not automatically workflow completion.",
            study.selected.len(),
            study
                .routes
                .iter()
                .map(|(name, count)| (name.clone(), *count as f64)),
        ),
        quantity(
            "Q03",
            "Delivery composition",
            "Selected deliveries",
            "tasks",
            "Fixed public categories from an exact-label allowlist; other project markers are redacted into a generic category, with no worker or model ranking.",
            study.selected.len(),
            domain_counts(study),
        ),
        quantity(
            "Q04",
            "Acquisition coverage",
            "Authenticated, publication-admitted historical export",
            "facts",
            "Ordered typed facts in the consumed vocabulary at or below the fixed horizon. Unrelated ingestion and unfiled Draft facts are excluded.",
            study.manifest.fact_count,
            [(
                "Admitted retained facts".to_owned(),
                study.manifest.fact_count as f64,
            )],
        ),
        quantity(
            "Q05",
            "Phase follow-up coverage",
            "Non-review claim admissions in the delivery calendar window",
            "episodes",
            "Eligibility requires admission no later than the horizon minus 24 hours. Missing history and late right-censoring are distinct.",
            outcomes.eligible + outcomes.right_censored,
            [
                (
                    "Eligible, covered".to_owned(),
                    (outcomes.eligible - outcomes.missing_history) as f64,
                ),
                (
                    "Missing history".to_owned(),
                    outcomes.missing_history as f64,
                ),
                (
                    "Late, right-censored".to_owned(),
                    outcomes.right_censored as f64,
                ),
            ],
        ),
    ];
    result
        .last_mut()
        .expect("follow-up quantity just added")
        .observed = outcomes.eligible - outcomes.missing_history;
    result
        .last_mut()
        .expect("follow-up quantity just added")
        .missing = outcomes.missing_history;
    result
        .last_mut()
        .expect("follow-up quantity just added")
        .censored = outcomes.right_censored;
    result.extend(phase_outcomes(outcomes));
    result.extend(waiting(study)?);
    result.extend(held_and_gaps(outcomes));
    result.extend(reviews(study));
    result.extend(calendar(study)?);
    result.extend(withdrawals(study));
    result.push(quantity("Q19", "Recorded materialization elapsed time", "One attributed historical experiment, not the admission population", "seconds",
        "Recorded in the curated materialization commit on the same packages, target directory and machine. These are not newly run benchmarks.", MATERIALIZATION_SECONDS.len(),
        MATERIALIZATION_LABELS.into_iter().zip(MATERIALIZATION_SECONDS).map(|(label, value)| (label.to_owned(), value))));
    result.push(quantity(
        "Q20",
        "Recorded materialization recompilation",
        "The same attributed experiment as Q19",
        "crates",
        "Crates recompiled in each recorded arm. No change to validation obligations is implied.",
        MATERIALIZATION_CRATES.len(),
        MATERIALIZATION_LABELS
            .into_iter()
            .zip(MATERIALIZATION_CRATES)
            .map(|(label, value)| (label.to_owned(), value as f64)),
    ));
    let seals = study
        .lifecycle
        .iter()
        .filter(|fact| fact.kind() == wire::FactKind::ProofSealed)
        .count();
    result.push(quantity("Q30", "Retained proof-seal observations", "Proof-seal facts retained in the admitted consumed history", "events",
        "A retained event count, not a denominator for historical proof executions, cost or reuse. Missing inventories and receipt GC do not imply zero proof activity.", seals,
        [("Retained seal facts".to_owned(), seals as f64)]));
    Ok(result)
}

fn domain_counts(study: &Study) -> BTreeMap<String, f64> {
    let mut counts = BTreeMap::new();
    for (project, count) in &study.projects {
        *counts.entry(domain(project).to_owned()).or_default() += *count as f64;
    }
    counts
}

fn phase_outcomes(outcomes: &Outcomes) -> Vec<Quantity> {
    let mut counts = BTreeMap::<String, f64>::new();
    let mut composition = BTreeMap::<String, f64>::new();
    for episode in outcomes.episodes.iter().filter(|episode| episode.eligible) {
        let period = regime(episode.claim.get());
        *counts
            .entry(format!(
                "{period} / {} / {}",
                phase(&episode.phase),
                episode.outcome
            ))
            .or_default() += 1.0;
        *composition
            .entry(format!("{period} / {}", phase(&episode.phase)))
            .or_default() += 1.0;
    }
    let mut outcomes_record = quantity(
        "Q06",
        "First phase outcome within 24 hours",
        "Eligible observed non-review phase admissions",
        "episodes",
        "First phase completion, release, lapse, derived different-lane transfer, deletion or operator closure within the common follow-up. Same-lane carries and rebounds preserve the episode. Still-held and missing history stay in the denominator.",
        outcomes.eligible,
        counts,
    );
    outcomes_record.missing = outcomes.missing_history;
    outcomes_record.censored = outcomes.right_censored;
    outcomes_record.exclusions = "Review claims; opening admissions before the delivery window; late admissions (separately right-censored in Q05). Periods are integration-aligned observation windows, not proved activation regimes.".to_owned();
    vec![
        outcomes_record,
        quantity(
            "Q07",
            "Phase exposure and composition",
            "Same eligible admissions as Q06",
            "episodes",
            "Phase admission counts beside each period comparison. Different phase composition and overlapping changes preclude attribution to one capability.",
            outcomes.eligible,
            composition,
        ),
    ]
}

fn waiting(study: &Study) -> Result<Vec<Quantity>> {
    let start = study.earliest_delivery.get().as_millisecond();
    let end = study.manifest.horizon_at.get().as_millisecond();
    let mut values = BTreeMap::<String, Vec<f64>>::new();
    let mut coverage = BTreeMap::<String, f64>::new();
    let mut admitted = 0;
    let mut late = 0;
    let mut missing = 0;
    for task in study.tasks.values().filter(|task| task.project != ".oops") {
        let at = task.admission.at.get().as_millisecond();
        if at < start || at > end {
            continue;
        }
        admitted += 1;
        if at + FOLLOW_UP_MS > end {
            late += 1;
            *coverage
                .entry("Late admission, right-censored".to_owned())
                .or_default() += 1.0;
            continue;
        }
        let period = regime(task.admission.seq.get());
        match task
            .first_claim
            .as_ref()
            .map(|claim| claim.at.get().as_millisecond())
        {
            Some(first) if first < at => {
                missing += 1;
                *coverage
                    .entry("Invalid recorded clock order".to_owned())
                    .or_default() += 1.0;
            }
            Some(first) if first <= at + FOLLOW_UP_MS => {
                values
                    .entry(format!("{period} / {}", phase(&task.admission.phase)))
                    .or_default()
                    .push((first - at) as f64 / MINUTE_MS);
                *coverage
                    .entry("First claim within 24h".to_owned())
                    .or_default() += 1.0;
            }
            _ => {
                *coverage
                    .entry("No first claim within 24h".to_owned())
                    .or_default() += 1.0;
            }
        }
    }
    let measured = values.values().map(Vec::len).sum();
    let samples = values
        .iter()
        .map(|(group, values)| (group.clone(), values.len()))
        .collect();
    let mut distribution = quantity(
        "Q08",
        "Admission-to-first-claim waiting",
        "Task admissions inside the window with a first claim in the common follow-up",
        "minutes",
        "Admission-defined task groups, not lifetime durations of completion-selected work. Quantiles condition on first claim within 24 hours; the companion coverage record prevents reading them as all-admission medians.",
        admitted - late,
        summary_groups(values),
    );
    distribution.observed = measured;
    distribution.missing = missing;
    distribution.censored = late;
    distribution.group_samples = samples;
    distribution.exclusions = "Legacy retained friction rows; late admissions; no first claim within follow-up; invalid clocks. Q09 supplies their coverage rather than treating absence as zero waiting.".to_owned();
    Ok(vec![
        distribution,
        quantity(
            "Q09",
            "Waiting coverage",
            "All public non-friction task admissions in the calendar window",
            "tasks",
            "Complete admission denominator for Q08; unresolved waiting and late right-censoring remain explicit.",
            admitted,
            coverage,
        ),
    ])
}

fn held_and_gaps(outcomes: &Outcomes) -> Vec<Quantity> {
    let mut held = BTreeMap::<String, Vec<f64>>::new();
    let mut by_task = BTreeMap::<&str, Vec<_>>::new();
    for episode in &outcomes.episodes {
        by_task.entry(&episode.task).or_default().push(episode);
        if !episode.eligible || episode.outcome == "missing history" {
            continue;
        }
        if let Some(end) = episode.ended {
            let duration = end.get().as_millisecond() - episode.admitted.get().as_millisecond();
            if (0..=FOLLOW_UP_MS).contains(&duration) {
                held.entry(format!(
                    "{} / {}",
                    regime(episode.claim.get()),
                    phase(&episode.phase)
                ))
                .or_default()
                .push(duration as f64 / MINUTE_MS);
            }
        }
    }
    let measured = held.values().map(Vec::len).sum();
    let held_samples = held
        .iter()
        .map(|(group, values)| (group.clone(), values.len()))
        .collect();
    let mut held_record = quantity(
        "Q10",
        "Claim-held intervals",
        "Eligible covered episodes ending within the follow-up",
        "minutes",
        "Wall time from canonical lane claim to its first terminal boundary. Held time is not CPU time, active labor, or provider-session duration.",
        outcomes.eligible,
        summary_groups(held),
    );
    held_record.observed = measured;
    held_record.missing = outcomes.missing_history;
    held_record.censored = outcomes.right_censored;
    held_record.group_samples = held_samples;
    let mut gaps = BTreeMap::<String, Vec<f64>>::new();
    let mut unknown = 0;
    for episodes in by_task.values_mut() {
        episodes.sort_by_key(|episode| episode.claim);
        for pair in episodes.windows(2) {
            let [previous, next] = pair else {
                unreachable!()
            };
            if !next.eligible {
                continue;
            }
            if let Some(end) = previous.ended {
                let gap = next.admitted.get().as_millisecond() - end.get().as_millisecond();
                if gap >= 0 && previous.outcome != "missing history" {
                    gaps.entry(phase(&next.phase).to_owned())
                        .or_default()
                        .push(gap as f64 / MINUTE_MS);
                } else {
                    unknown += 1;
                }
            } else {
                unknown += 1;
            }
        }
    }
    let gaps_observed = gaps.values().map(Vec::len).sum::<usize>();
    let gap_samples = gaps
        .iter()
        .map(|(group, values)| (group.clone(), values.len()))
        .collect();
    let mut gap_record = quantity(
        "Q11",
        "Inter-claim gaps",
        "Successive same-task non-review episodes, with the later admission eligible",
        "minutes",
        "Time between a recorded predecessor terminal boundary and the next claim. Derived transfers can yield a genuine zero gap; absent predecessors never become invented zeroes.",
        gaps_observed + unknown,
        summary_groups(gaps),
    );
    gap_record.observed = gaps_observed;
    gap_record.missing = unknown;
    gap_record.group_samples = gap_samples;
    vec![held_record, gap_record]
}

fn reviews(study: &Study) -> Vec<Quantity> {
    let selected = study
        .selected
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    let mut first = BTreeMap::<String, f64>::new();
    let mut rounds = BTreeMap::<String, f64>::new();
    let mut attribution = BTreeMap::<String, f64>::new();
    let mut corrections = BTreeMap::<String, f64>::new();
    let mut review_count = 0;
    let mut reviewed_tasks = 0;
    let mut any_findings = 0;
    for task in study
        .tasks
        .values()
        .filter(|task| selected.contains(&task.birth))
    {
        if let Some(review) = task.reviews.first() {
            reviewed_tasks += 1;
            *first
                .entry(format!(
                    "{} / first {}",
                    regime(review.observation.seq.get()),
                    review.verdict
                ))
                .or_default() += 1.0;
        }
        if task
            .reviews
            .iter()
            .any(|review| review.verdict == "finding")
        {
            any_findings += 1;
        }
        if !task.reviews.is_empty() {
            *rounds
                .entry(format!("{} ordered verdict(s)", task.reviews.len()))
                .or_default() += 1.0;
        }
        for review in &task.reviews {
            review_count += 1;
            *attribution
                .entry(review_attribution(review).to_owned())
                .or_default() += 1.0;
            for child in review
                .correction_births
                .iter()
                .filter_map(|birth| study.tasks.get(birth))
            {
                let flow = child
                    .flow
                    .iter()
                    .map(|value| phase(value))
                    .collect::<Vec<_>>()
                    .join(" → ");
                *corrections.entry(format!("Recorded {flow}")).or_default() += 1.0;
            }
        }
    }
    first.insert(
        "No recorded review verdict".to_owned(),
        (study.selected.len() - reviewed_tasks) as f64,
    );
    let corrections_total = corrections.values().sum::<f64>() as usize;
    vec![
        quantity(
            "Q12",
            "First review verdict",
            "Selected deliveries, assigned at the first verdict boundary",
            "tasks",
            "A final clean closure does not establish first-pass success. No-review tasks and the legacy review-phase-done route remain visible.",
            study.selected.len(),
            first,
        ),
        quantity(
            "Q13",
            "Ordered review rounds",
            "Selected tasks with one or more review verdicts",
            "tasks",
            "Rounds preserve sequence order; they are not inferred from the current clean head.",
            reviewed_tasks,
            rounds,
        ),
        quantity(
            "Q14",
            "Any finding versus no finding",
            "Selected reviewed deliveries",
            "tasks",
            "At least one finding anywhere in the ordered review history.",
            reviewed_tasks,
            [
                ("Any recorded finding".to_owned(), any_findings as f64),
                (
                    "No recorded finding".to_owned(),
                    (reviewed_tasks - any_findings) as f64,
                ),
            ],
        ),
        quantity(
            "Q15",
            "Reviewer versus phase author",
            "All verdicts on selected deliveries",
            "verdicts",
            "Reviewer joined to the preceding completed non-review phase author; creator is separate. Different lane is not proof of human or model independence.",
            review_count,
            attribution,
        ),
        quantity(
            "Q16",
            "Correction-task recorded flows",
            "Correction births causally linked to findings on selected parents",
            "tasks",
            "Resulting journal flow is distinct from requested flow. Only the curated command-record case proves an explicit operand.",
            corrections_total,
            corrections,
        ),
    ]
}

/// Reviewer joined to the preceding non-review phase author, by recorded lane.
fn review_attribution(review: &super::reduce::Review) -> &'static str {
    let is_lane = |actor: &serde_json::Value| {
        actor
            .as_str()
            .is_some_and(|actor| actor.starts_with("lane:"))
    };
    match &review.author {
        Some(author) if author == &review.observation.actor => "Same lane",
        Some(author) if is_lane(author) && is_lane(&review.observation.actor) => "Different lane",
        _ => "Unclassified attribution",
    }
}

fn calendar(study: &Study) -> Result<Vec<Quantity>> {
    let first = study
        .earliest_delivery
        .get()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date();
    let last = study
        .manifest
        .horizon_at
        .get()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date();
    let mut closures = BTreeMap::<String, f64>::new();
    let mut date = first;
    loop {
        closures.insert(date.to_string(), 0.0);
        if date == last {
            break;
        }
        date = date.tomorrow()?;
    }
    for birth in &study.selected {
        let closure = study.tasks[birth]
            .closure
            .as_ref()
            .ok_or("selected task lacks closure")?;
        *closures
            .get_mut(&day(&closure.at.to_string()))
            .ok_or("selected closure is outside the calendar window")? += 1.0;
    }
    Ok(vec![quantity(
        "Q17",
        "Selected delivery calendar",
        "Selected deliveries by recorded closure day",
        "tasks",
        "UTC days, including zero-count days. This completion-selected burnup is not backlog or WIP.",
        study.selected.len(),
        closures,
    )])
}

fn withdrawals(study: &Study) -> Vec<Quantity> {
    let mut counts = BTreeMap::<String, f64>::new();
    let mut admissions = 0;
    for task in study
        .tasks
        .values()
        .filter(|task| task.admission.at >= study.earliest_delivery && task.project != ".oops")
    {
        admissions += 1;
        let label = if task.deleted && task.first_claim.is_none() {
            "Deleted before first claim"
        } else if task.deleted {
            "Deleted after a recorded claim"
        } else if task.closure.is_none() {
            "Incomplete at horizon"
        } else {
            "Completed at horizon"
        };
        *counts.entry(label.to_owned()).or_default() += 1.0;
    }
    let mut record = quantity(
        "Q18",
        "Admission disposition at the horizon",
        "Public non-friction task admissions in the calendar window",
        "tasks",
        "Calendar admission population includes incomplete and deleted work. Deletion is not completion; hidden friction and legacy friction rows are excluded from withdrawal rates.",
        admissions,
        counts,
    );
    record.exclusions = "Publication-inadmissible hidden tasks; retained legacy friction rows. Completion selection does not define this denominator.".to_owned();
    vec![record]
}

pub(super) fn summary_groups(mut groups: BTreeMap<String, Vec<f64>>) -> Vec<(String, f64)> {
    let mut rows = Vec::new();
    for (group, values) in &mut groups {
        if values.is_empty() {
            continue;
        }
        values.sort_by(f64::total_cmp);
        rows.push((format!("{group} / median"), quantile(values, MEDIAN)));
        rows.push((format!("{group} / p90"), quantile(values, P90)));
        rows.push((format!("{group} / p95"), quantile(values, P95)));
    }
    rows
}

pub(super) fn quantile(values: &[f64], probability: f64) -> f64 {
    assert!(
        !values.is_empty(),
        "an absent distribution is not zero duration"
    );
    let index = ((values.len() - 1) as f64 * probability).ceil() as usize;
    values[index]
}
