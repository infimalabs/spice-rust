use super::super::execute_action;
use super::*;

/// An odd paired sample lets one host-load outlier pass while two regressing
/// observations still determine the median. Alternating launch order prevents
/// a monotonic warm-host drift from consistently favoring either route.
const BENCHMARK_PAIR_COUNT: usize = 3;
const _: () = assert!(BENCHMARK_PAIR_COUNT >= 3 && BENCHMARK_PAIR_COUNT % 2 == 1);
const MAX_WALL_RATIO: f64 = 0.75;
const MAX_CPU_RATIO: f64 = 1.15;
const BENCHMARK_MARKER: &str = "verifier-benchmark wall_nanos=";
const SERIAL_BENCHMARK_TEST: &str =
    "verify_execution::tests::benchmark::warm_serial_full_verifier_benchmark_fixture";
const CONCURRENT_BENCHMARK_TEST: &str =
    "verify_execution::tests::benchmark::warm_concurrent_full_verifier_benchmark_fixture";

#[test]
fn controlled_warm_benchmark_cuts_wall_time_without_more_cpu_work() {
    warm_benchmark_process(SERIAL_BENCHMARK_TEST);
    warm_benchmark_process(CONCURRENT_BENCHMARK_TEST);
    let pairs = (0..BENCHMARK_PAIR_COUNT)
        .map(|ordinal| benchmark_pair(ordinal % 2 == 0))
        .collect::<Vec<_>>();
    let wall_ratio = median(
        pairs
            .iter()
            .map(|(serial, concurrent)| concurrent.wall_nanos as f64 / serial.wall_nanos as f64)
            .collect(),
    );
    let cpu_ratio = median(
        pairs
            .iter()
            .map(|(serial, concurrent)| concurrent.cpu_seconds / serial.cpu_seconds)
            .collect(),
    );
    let evidence = pairs
        .iter()
        .enumerate()
        .map(|(ordinal, (serial, concurrent))| {
            format!(
                concat!(
                    "pair={} serial_wall_ns={} concurrent_wall_ns={} ",
                    "serial_cpu_seconds={:.3} concurrent_cpu_seconds={:.3}"
                ),
                ordinal + 1,
                serial.wall_nanos,
                concurrent.wall_nanos,
                serial.cpu_seconds,
                concurrent.cpu_seconds,
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    println!(
        "[verify-benchmark] median_wall_ratio={wall_ratio:.3} median_cpu_ratio={cpu_ratio:.3} {evidence}"
    );

    assert!(
        wall_ratio <= MAX_WALL_RATIO,
        "median warm scheduler reduction was below 25%: median_ratio={wall_ratio:.3}; {evidence}",
    );
    assert!(
        cpu_ratio <= MAX_CPU_RATIO,
        "median measured process CPU grew over 15%: median_ratio={cpu_ratio:.3}; {evidence}",
    );
}

#[test]
fn benchmark_median_tolerates_one_outlier_but_not_two_regressions() {
    assert_eq!(median(vec![0.91, 0.70, 0.72]), 0.72);
    assert!(median(vec![0.91, 0.70, 0.72]) <= MAX_WALL_RATIO);
    assert!(median(vec![0.91, 0.79, 0.72]) > MAX_WALL_RATIO);
}

#[test]
#[ignore = "launched exactly by the measured parent benchmark"]
fn warm_serial_full_verifier_benchmark_fixture() {
    emit_benchmark(BenchmarkMode::Serial);
}

#[test]
#[ignore = "launched exactly by the measured parent benchmark"]
fn warm_concurrent_full_verifier_benchmark_fixture() {
    emit_benchmark(BenchmarkMode::Concurrent);
}

#[derive(Clone, Copy)]
enum BenchmarkMode {
    Serial,
    Concurrent,
}

struct BenchmarkMeasurement {
    wall_nanos: u128,
    cpu_seconds: f64,
}

fn benchmark_pair(serial_first: bool) -> (BenchmarkMeasurement, BenchmarkMeasurement) {
    if serial_first {
        (
            benchmark_process(SERIAL_BENCHMARK_TEST),
            benchmark_process(CONCURRENT_BENCHMARK_TEST),
        )
    } else {
        let concurrent = benchmark_process(CONCURRENT_BENCHMARK_TEST);
        let serial = benchmark_process(SERIAL_BENCHMARK_TEST);
        (serial, concurrent)
    }
}

fn median(mut values: Vec<f64>) -> f64 {
    assert!(!values.is_empty() && values.len() % 2 == 1);
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn emit_benchmark(mode: BenchmarkMode) {
    let measured = TimedRunner::benchmark();
    let started = Instant::now();
    run_benchmark_route(&measured, mode);
    let wall_nanos = started.elapsed().as_nanos();
    assert_exactly_once(&measured.calls());
    println!("{BENCHMARK_MARKER}{wall_nanos}");
}

fn run_benchmark_route(runner: &TimedRunner, mode: BenchmarkMode) {
    match mode {
        BenchmarkMode::Serial => {
            let cancellation = CancellationToken::new();
            for action in VERIFY_ACTIONS {
                execute_action(runner, action, &cancellation, Budget::fixture(3, 2, 0))
                    .result
                    .expect("serial fixture action succeeds");
            }
        }
        BenchmarkMode::Concurrent => verify_with_control(
            runner,
            &mut Vec::new(),
            Budget::fixture(3, 2, 0),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .expect("concurrent fixture action graph succeeds"),
    }
}

fn benchmark_process(test: &str) -> BenchmarkMeasurement {
    let executable = std::env::current_exe().expect("resolve the xtask unit-test process");
    let arguments = benchmark_arguments(test);
    let output = Command::new("/usr/bin/time")
        .args(["-p"])
        .arg(executable)
        .args(arguments)
        .output()
        .expect("launch one exact benchmark fixture through the process CPU meter");
    assert!(
        output.status.success(),
        "benchmark fixture {test} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).expect("benchmark stdout is UTF-8");
    let wall_nanos = stdout
        .lines()
        .find_map(|line| {
            line.find(BENCHMARK_MARKER).and_then(|offset| {
                line[offset + BENCHMARK_MARKER.len()..]
                    .trim()
                    .parse::<u128>()
                    .ok()
            })
        })
        .unwrap_or_else(|| panic!("benchmark emitted no wall measurement: {stdout}"));
    let stderr = String::from_utf8(output.stderr).expect("benchmark stderr is UTF-8");
    let user = time_value(&stderr, "user ");
    let system = time_value(&stderr, "sys ");
    let cpu_seconds = user + system;
    assert!(
        cpu_seconds > 0.0,
        "benchmark measured no process CPU: {stderr}"
    );
    BenchmarkMeasurement {
        wall_nanos,
        cpu_seconds,
    }
}

fn warm_benchmark_process(test: &str) {
    let executable = std::env::current_exe().expect("resolve the xtask unit-test process");
    let output = Command::new(executable)
        .args(benchmark_arguments(test))
        .output()
        .expect("launch one exact warm benchmark fixture");
    assert!(
        output.status.success(),
        "warm benchmark fixture {test} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn time_value(output: &str, label: &str) -> f64 {
    output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(label)?.parse::<f64>().ok())
        .unwrap_or_else(|| panic!("/usr/bin/time emitted no {label:?} value: {output}"))
}
