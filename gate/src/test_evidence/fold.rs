//! Pure comparisons over validated test-evidence graph values.

use std::collections::BTreeMap;

use crate::kill::SurvivorRate;

use super::{SubsumptionRelation, TestCoverage, TestSubsumptionFinding};

pub(super) fn subsumption_findings(tests: &[TestCoverage]) -> Vec<TestSubsumptionFinding> {
    let features = tests
        .iter()
        .map(|test| (test, test.features()))
        .filter(|(_, features)| !features.is_empty())
        .collect::<Vec<_>>();
    let mut groups = BTreeMap::<Vec<(String, u32)>, Vec<&TestCoverage>>::new();
    for (test, points) in &features {
        groups
            .entry(points.iter().cloned().collect())
            .or_default()
            .push(test);
    }
    for members in groups.values_mut() {
        members.sort_by(|left, right| left.test.cmp(&right.test));
    }

    let mut findings = Vec::new();
    for members in groups.values().filter(|members| members.len() > 1) {
        let representative = members[0];
        findings.extend(members[1..].iter().map(|test| TestSubsumptionFinding {
            test: test.test.clone(),
            contained_by: representative.test.clone(),
            relation: SubsumptionRelation::Equal,
            covered_sources: test.sources.clone(),
        }));
    }
    for members in groups.values().filter(|members| members.len() == 1) {
        let test = members[0];
        let own = test.features();
        let representative = features
            .iter()
            .filter(|(other, other_features)| other.test != test.test && own < *other_features)
            .min_by(|(left, left_features), (right, right_features)| {
                (left_features.len() - own.len(), left.test.as_str())
                    .cmp(&(right_features.len() - own.len(), right.test.as_str()))
            })
            .map(|(test, _)| *test);
        if let Some(representative) = representative {
            findings.push(TestSubsumptionFinding {
                test: test.test.clone(),
                contained_by: representative.test.clone(),
                relation: SubsumptionRelation::StrictSubset,
                covered_sources: test.sources.clone(),
            });
        }
    }
    findings.sort_by(|left, right| {
        (&left.contained_by, &left.test).cmp(&(&right.contained_by, &right.test))
    });
    findings
}

pub(super) fn rate_regressed(measured: SurvivorRate, floor: SurvivorRate) -> bool {
    u128::from(measured.numerator()) * u128::from(floor.denominator())
        > u128::from(floor.numerator()) * u128::from(measured.denominator())
}
