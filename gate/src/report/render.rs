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
