fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn render_process(rendered: &mut String, prefix: &str, process: &ReportProcess) {
    match &process.completion {
        ReportCompletion::Exited { code } => {
            let _ = writeln!(rendered, "{prefix} completion exited {}", code_text(*code));
        }
        ReportCompletion::TimedOut { code } => {
            let _ = writeln!(
                rendered,
                "{prefix} completion timed-out {}",
                code_text(*code)
            );
        }
        ReportCompletion::Cancelled { code } => {
            let _ = writeln!(
                rendered,
                "{prefix} completion cancelled {}",
                code_text(*code)
            );
        }
        ReportCompletion::LaunchError { evidence } => {
            let _ = writeln!(
                rendered,
                "{prefix} completion launch-error {}",
                quoted(evidence)
            );
        }
        ReportCompletion::WaitError { evidence } => {
            let _ = writeln!(
                rendered,
                "{prefix} completion wait-error {}",
                quoted(evidence)
            );
        }
        ReportCompletion::SpecError { evidence } => {
            let _ = writeln!(
                rendered,
                "{prefix} completion spec-error {}",
                quoted(evidence)
            );
        }
    }
    render_stream(rendered, prefix, "stdout", &process.stdout);
    render_stream(rendered, prefix, "stderr", &process.stderr);
}

fn code_text(code: Option<i32>) -> String {
    code.map_or_else(|| "none".to_owned(), |code| code.to_string())
}

fn render_stream(rendered: &mut String, prefix: &str, name: &str, stream: &ReportStream) {
    match stream {
        ReportStream::Unavailable => {
            let _ = writeln!(rendered, "{prefix} {name} unavailable");
        }
        ReportStream::Captured {
            bytes,
            truncated,
            complete,
        } => {
            let _ = writeln!(
                rendered,
                "{prefix} {name} captured truncated {truncated} complete {complete} bytes {}",
                hex(bytes)
            );
        }
    }
}

fn render_outcome(rendered: &mut String, prefix: &str, outcome: &ReportOutcome) {
    match outcome {
        ReportOutcome::Present { bytes } => {
            let _ = writeln!(rendered, "{prefix} outcome present bytes {}", hex(bytes));
        }
        ReportOutcome::Absent => {
            let _ = writeln!(rendered, "{prefix} outcome absent");
        }
        ReportOutcome::Unreadable { evidence } => {
            let _ = writeln!(rendered, "{prefix} outcome unreadable {}", quoted(evidence));
        }
    }
}

fn render_optional_rate(rendered: &mut String, label: &str, rate: Option<ReportRate>) {
    match rate {
        None => {
            let _ = writeln!(rendered, "{label} none");
        }
        Some(rate) => {
            let _ = writeln!(rendered, "{label} {}/{}", rate.numerator, rate.denominator);
        }
    }
}

fn render_movement(rendered: &mut String, movement: ReportMutationMovement) {
    match movement {
        ReportMutationMovement::Held { floor } => {
            let _ = writeln!(
                rendered,
                "movement held {}/{}",
                floor.numerator, floor.denominator
            );
        }
        ReportMutationMovement::Shrank { floor, measured } => {
            let _ = writeln!(
                rendered,
                "movement shrank {}/{} to {}/{}",
                floor.numerator, floor.denominator, measured.numerator, measured.denominator
            );
        }
        ReportMutationMovement::Regressed { floor, measured } => {
            let _ = writeln!(
                rendered,
                "movement regressed {}/{} to {}/{}",
                floor.numerator, floor.denominator, measured.numerator, measured.denominator
            );
        }
        ReportMutationMovement::Unbaselined { measured } => {
            let _ = writeln!(
                rendered,
                "movement unbaselined {}/{}",
                measured.numerator, measured.denominator
            );
        }
        ReportMutationMovement::Unconfirmed { confirmed, total } => {
            let _ = writeln!(rendered, "movement unconfirmed {confirmed}/{total}");
        }
    }
}

/// Name every metric refusing this tree, and the files behind each one.
///
/// Public because two callers need exactly these words, and only one of them
/// can see this crate's insides. The report fixtures adjudicate the live
/// workspace from within; `tracked_inputs_load_one_operational_current_contract`
/// in `gate/tests/input.rs` adjudicates the same root from outside, and without
/// this would fail with a bare standing that names no metric and no path. One
/// copy is what stops the two explanations from drifting apart.
///
/// A refusal here says nothing about an earlier compiler or test step having
/// passed: those steps can be green on a tree these assertions reject.
/// `cargo xtask verify` reaches this battery at contract landing after its
/// Cargo commands; only the battery reads the ratchet against the working tree.
///
/// Built only from the contract's own declarations: the per-file counts come
/// from re-running each metric's declared parser, so this attribution cannot
/// drift from what the gate measured. Every carrying file is listed and none
/// is dropped, heaviest first only so the order is stable to read: the file
/// that moved a baseline is often not the file that holds the most, so the
/// top of this list is where to start looking rather than the answer.
#[must_use]
pub fn refusal_detail(root: &Path, contract: &Contract, report: &GateReport) -> String {
    let mut detail = String::from(
        "the live workspace is refused, so this run carries neither a clean \
         standing nor a tightening proposal. a passing compiler or test step \
         does not contradict this: this battery is what reads the ratchet.",
    );
    for finding in report
        .findings()
        .iter()
        .filter(|finding| !finding.movement.passes())
    {
        let _ = write!(detail, "\n\n{} {}", finding.metric, finding.movement);
        let Some(metric) = contract
            .policy()
            .metrics()
            .iter()
            .find(|metric| metric.name() == finding.metric)
        else {
            let _ = write!(
                detail,
                "\n  nothing declares this metric, so the baseline is what moves"
            );
            continue;
        };
        let _ = write!(detail, "\n  counts {}", metric.target());
        let Some(measurement) = report
            .measurements()
            .iter()
            .find(|measurement| measurement.metric() == finding.metric)
        else {
            continue;
        };
        let mut carried: Vec<(u64, &str)> = measurement
            .files()
            .iter()
            .filter_map(|file| {
                let count = metric_count(root, metric, file)?;
                (count > 0).then_some((count, file.as_str()))
            })
            .collect();
        carried.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(right.1)));
        let attributed: u64 = carried.iter().map(|(count, _)| count).sum();
        if attributed == measurement.value() {
            let _ = write!(
                detail,
                "\n  {} of {} measured files hold all {}:",
                carried.len(),
                measurement.files().len(),
                measurement.value()
            );
        } else {
            let _ = write!(
                detail,
                "\n  these total {attributed}, not the measured {}, so read them \
                 as where to look:",
                measurement.value()
            );
        }
        for (count, file) in carried {
            let _ = write!(detail, "\n    {count} {file}");
        }
    }
    detail
}

/// Re-run one metric's declared parser over a single file.
///
/// Returns nothing when the parser cannot be run or does not print a count;
/// such a file drops out of the attribution, which the total then reports as
/// a shortfall rather than passing off as a clean breakdown.
fn metric_count(root: &Path, metric: &Metric, file: &str) -> Option<u64> {
    let output = Command::new(metric.parser().program())
        .args(metric.parser().arguments())
        .arg(file)
        .current_dir(root)
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}
