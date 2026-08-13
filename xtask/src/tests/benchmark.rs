use super::super::execute_action;
use super::*;

const PERCENT_SCALE: u64 = 100;
const CPU_PERCENT_SCALE: f64 = 100.0;
const BENCHMARK_MARKER: &str = "verifier-benchmark wall_nanos=";
const SERIAL_BENCHMARK_TEST: &str =
    "verify_execution::tests::benchmark::warm_serial_full_verifier_benchmark_fixture";
const CONCURRENT_BENCHMARK_TEST: &str =
    "verify_execution::tests::benchmark::warm_concurrent_full_verifier_benchmark_fixture";

#[test]
fn controlled_warm_benchmark_cuts_wall_time_without_more_cpu_work() {
    let serial = benchmark_process(SERIAL_BENCHMARK_TEST);
    let concurrent = benchmark_process(CONCURRENT_BENCHMARK_TEST);
    println!(
        concat!(
            "[verify-benchmark] serial_wall_ns={} concurrent_wall_ns={} ",
            "serial_cpu_seconds={:.3} concurrent_cpu_seconds={:.3}"
        ),
        serial.wall_nanos, concurrent.wall_nanos, serial.cpu_seconds, concurrent.cpu_seconds,
    );

    assert!(
        concurrent.wall_nanos * u128::from(PERCENT_SCALE) <= serial.wall_nanos * 75,
        concat!(
            "warm scheduler reduction was below 25%: ",
            "serial_ns={serial_wall} concurrent_ns={concurrent_wall}"
        ),
        serial_wall = serial.wall_nanos,
        concurrent_wall = concurrent.wall_nanos,
    );
    assert!(
        concurrent.cpu_seconds * CPU_PERCENT_SCALE <= serial.cpu_seconds * 115.0,
        "measured process CPU grew over 15%: serial={:.3}s concurrent={:.3}s",
        serial.cpu_seconds,
        concurrent.cpu_seconds,
    );
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
    let warm = Command::new(&executable)
        .args(arguments)
        .output()
        .expect("launch one exact warm benchmark fixture");
    assert!(
        warm.status.success(),
        "warm benchmark fixture {test} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&warm.stdout),
        String::from_utf8_lossy(&warm.stderr),
    );
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

fn time_value(output: &str, label: &str) -> f64 {
    output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(label)?.parse::<f64>().ok())
        .unwrap_or_else(|| panic!("/usr/bin/time emitted no {label:?} value: {output}"))
}
