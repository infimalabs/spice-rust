//! Explicit selection and timing for repository-classified slow tests.

use std::collections::BTreeSet;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Write};
use std::process::Command;
use std::time::Instant;

use serde::Deserialize;

const REGISTRY: &str = ::std::include_str!("../../test-classification.json");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    version: u64,
    tests: Vec<ClassifiedTest>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassifiedTest {
    id: String,
    class: TestClass,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    target: Vec<String>,
    #[serde(default)]
    test: Option<String>,
    #[serde(default)]
    ignored: bool,
    #[serde(default)]
    command: Vec<String>,
    reason: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum TestClass {
    Slow,
}

impl ClassifiedTest {
    fn argv(&self) -> Result<Vec<OsString>, ClassificationError> {
        if !self.command.is_empty() {
            return Ok(self.command.iter().map(OsString::from).collect());
        }
        let package = self.package.as_deref().ok_or_else(|| {
            ClassificationError::Registry(format!("test {:?} has no package or command", self.id))
        })?;
        let test = self.test.as_deref().ok_or_else(|| {
            ClassificationError::Registry(format!("test {:?} has no exact test name", self.id))
        })?;
        let mut argv = ["cargo", "test", "--locked", "--package"]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        argv.push(package.into());
        argv.extend(self.target.iter().map(OsString::from));
        argv.push(test.into());
        argv.extend([OsString::from("--"), OsString::from("--exact")]);
        if self.ignored {
            argv.push(OsString::from("--ignored"));
        }
        argv.push(OsString::from("--nocapture"));
        Ok(argv)
    }
}

/// Print the canonical slow-test inventory without executing it.
pub fn list(output: &mut impl Write) -> Result<(), ClassificationError> {
    let registry = registry()?;
    for test in registry.tests {
        writeln!(
            output,
            "[test-class] class=slow id={} reason={:?}",
            test.id, test.reason
        )?;
    }
    Ok(())
}

/// Run one package's ordinary suite while excluding every registered slow test.
pub fn run_fast(package: &str, output: &mut impl Write) -> Result<(), ClassificationError> {
    let argv = fast_argv(package)?;
    let excluded = fast_excluded(package)?;
    let started = Instant::now();
    writeln!(
        output,
        "[test-class] stage=start class=fast package={} excluded_slow={} command={:?}",
        package, excluded, argv,
    )?;
    output.flush()?;
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .map_err(|source| ClassificationError::Launch {
            id: format!("fast:{package}"),
            source,
        })?;
    writeln!(
        output,
        "[test-class] stage=summary class=fast package={} excluded_slow={} accumulated_elapsed_ms={} success={}",
        package,
        excluded,
        started.elapsed().as_millis(),
        status.success(),
    )?;
    if status.success() {
        Ok(())
    } else {
        Err(ClassificationError::Exit {
            id: format!("fast:{package}"),
            code: status.code(),
        })
    }
}

pub(crate) fn fast_argv(package: &str) -> Result<Vec<OsString>, ClassificationError> {
    if package.is_empty()
        || !package
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ClassificationError::Selection(format!(
            "invalid fast-suite package {package:?}"
        )));
    }
    let registry = registry()?;
    let skips = registry
        .tests
        .iter()
        .filter(|test| test.package.as_deref() == Some(package))
        .filter_map(|test| test.test.as_deref())
        .collect::<Vec<_>>();
    let mut argv = ["cargo", "test", "--locked", "--package"]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    argv.push(package.into());
    argv.push("--all-targets".into());
    if !skips.is_empty() {
        argv.push("--".into());
        for test in &skips {
            argv.extend([OsString::from("--skip"), OsString::from(test)]);
        }
    }
    Ok(argv)
}

fn fast_excluded(package: &str) -> Result<usize, ClassificationError> {
    Ok(registry()?
        .tests
        .iter()
        .filter(|test| test.package.as_deref() == Some(package))
        .count())
}

/// Run exact named slow tests and report individual plus accumulated wall time.
pub fn run_slow(ids: &[String], output: &mut impl Write) -> Result<(), ClassificationError> {
    if ids.is_empty() {
        return Err(ClassificationError::Selection(
            "slow execution requires at least one exact registry id; use --list".to_owned(),
        ));
    }
    let registry = registry()?;
    let requested = ids.iter().collect::<BTreeSet<_>>();
    if requested.len() != ids.len() {
        return Err(ClassificationError::Selection(
            "slow test ids must be unique".to_owned(),
        ));
    }
    let selected = registry
        .tests
        .iter()
        .filter(|test| requested.contains(&test.id))
        .collect::<Vec<_>>();
    let found = selected
        .iter()
        .map(|test| &test.id)
        .collect::<BTreeSet<_>>();
    let missing = requested.difference(&found).collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(ClassificationError::Selection(format!(
            "unknown slow test id(s): {}",
            missing
                .into_iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    let total_started = Instant::now();
    for (index, test) in selected.iter().enumerate() {
        let argv = test.argv()?;
        let (program, arguments) = argv.split_first().expect("validated command is nonempty");
        writeln!(
            output,
            "[test-class] stage=start class=slow ordinal={}/{} id={} command={:?}",
            index + 1,
            selected.len(),
            test.id,
            argv,
        )?;
        output.flush()?;
        let started = Instant::now();
        let status = Command::new(program)
            .args(arguments)
            .status()
            .map_err(|source| ClassificationError::Launch {
                id: test.id.clone(),
                source,
            })?;
        let elapsed = started.elapsed().as_millis();
        writeln!(
            output,
            "[test-class] stage=terminal class=slow id={} elapsed_ms={} success={}",
            test.id,
            elapsed,
            status.success(),
        )?;
        output.flush()?;
        if !status.success() {
            return Err(ClassificationError::Exit {
                id: test.id.clone(),
                code: status.code(),
            });
        }
    }
    writeln!(
        output,
        "[test-class] stage=summary class=slow tests={} accumulated_elapsed_ms={}",
        selected.len(),
        total_started.elapsed().as_millis(),
    )?;
    Ok(())
}

fn registry() -> Result<Registry, ClassificationError> {
    let registry: Registry = serde_json::from_str(REGISTRY)
        .map_err(|error| ClassificationError::Registry(error.to_string()))?;
    if registry.version != 1 {
        return Err(ClassificationError::Registry(format!(
            "unsupported version {}",
            registry.version
        )));
    }
    let mut ids = BTreeSet::new();
    for test in &registry.tests {
        if test.class != TestClass::Slow
            || test.id.is_empty()
            || test.reason.is_empty()
            || !ids.insert(&test.id)
        {
            return Err(ClassificationError::Registry(format!(
                "invalid or duplicate slow test {:?}",
                test.id
            )));
        }
        let cargo = test.command.is_empty();
        if cargo != (test.package.is_some() && test.test.is_some())
            || (!cargo
                && (test.package.is_some() || test.test.is_some() || !test.target.is_empty()))
        {
            return Err(ClassificationError::Registry(format!(
                "test {:?} must declare exactly one Cargo or command selector",
                test.id
            )));
        }
    }
    Ok(registry)
}

#[derive(Debug)]
pub enum ClassificationError {
    Registry(String),
    Selection(String),
    Launch { id: String, source: io::Error },
    Exit { id: String, code: Option<i32> },
    Output(io::Error),
}

impl From<io::Error> for ClassificationError {
    fn from(source: io::Error) -> Self {
        Self::Output(source)
    }
}

impl fmt::Display for ClassificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(detail) => write!(formatter, "slow-test registry refused: {detail}"),
            Self::Selection(detail) => write!(formatter, "slow-test selection refused: {detail}"),
            Self::Launch { id, source } => {
                write!(formatter, "slow test {id:?} did not launch: {source}")
            }
            Self::Exit {
                id,
                code: Some(code),
            } => write!(formatter, "slow test {id:?} failed with exit {code}"),
            Self::Exit { id, code: None } => write!(formatter, "slow test {id:?} ended by signal"),
            Self::Output(source) => write!(formatter, "slow-test report failed: {source}"),
        }
    }
}

impl Error for ClassificationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Launch { source, .. } | Self::Output(source) => Some(source),
            Self::Registry(_) | Self::Selection(_) | Self::Exit { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_entries_have_runnable_slow_selectors() {
        let registry = registry().expect("classification registry is valid");
        assert!(
            registry
                .tests
                .iter()
                .all(|test| test.class == TestClass::Slow)
        );
        for test in registry.tests {
            let argv = test.argv().expect("classified test has an argv");
            assert!(!argv.is_empty());
            if test.command.is_empty() {
                assert!(argv.iter().any(|argument| argument == "--exact"));
            }
        }
    }

    #[test]
    fn fast_selection_excludes_every_package_owned_slow_test() {
        let registry = registry().expect("classification registry is valid");
        let packages = registry
            .tests
            .iter()
            .filter_map(|test| test.package.as_deref())
            .collect::<BTreeSet<_>>();
        for package in packages {
            let argv = fast_argv(package).expect("package fast selection");
            for test in registry
                .tests
                .iter()
                .filter(|test| test.package.as_deref() == Some(package))
            {
                let name = test.test.as_ref().expect("package entry has a test name");
                assert!(
                    argv.windows(2).any(|pair| {
                        pair[0] == "--skip" && pair[1] == std::ffi::OsStr::new(name)
                    }),
                    "fast selection for {package:?} did not exclude {:?}",
                    test.id,
                );
            }
        }
    }

    #[test]
    fn slow_execution_requires_explicit_unique_ids() {
        let mut output = Vec::new();
        assert!(
            run_slow(&[], &mut output)
                .unwrap_err()
                .to_string()
                .contains("at least one")
        );
        let repeated = vec!["canvas:browser".to_owned(), "canvas:browser".to_owned()];
        assert!(
            run_slow(&repeated, &mut output)
                .unwrap_err()
                .to_string()
                .contains("unique")
        );
    }
}
