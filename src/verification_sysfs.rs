use crate::profile::Profile;
use crate::profile_runtime;
use crate::tuning::generic_sysfs;
use crate::verification::{VerificationIssue, VerificationIssueKind, VerificationReport};

pub fn augment(profile: &Profile, report: &mut VerificationReport) {
    remove_placeholder_contract_issues(report);

    let units = match profile_runtime::active_units(profile) {
        Ok(units) => units,
        Err(error) => {
            push_issue(
                report,
                VerificationIssueKind::ReadError,
                "conditions",
                "runtime profile",
                "active sysfs units",
                None,
                error.to_string(),
            );
            return;
        }
    };

    for unit in units
        .into_iter()
        .filter(|unit| unit.plugin_type == "sysfs")
    {
        for (pattern, expected) in &unit.options {
            match generic_sysfs::expand_pattern(pattern) {
                Ok(targets) if targets.is_empty() => push_issue(
                    report,
                    VerificationIssueKind::Missing,
                    pattern,
                    pattern,
                    expected,
                    None,
                    "generic sysfs pattern matched no controls",
                ),
                Ok(targets) => {
                    for target in targets {
                        report.checked += 1;
                        match generic_sysfs::read_active_value(&target) {
                            Ok(actual) if generic_sysfs::value_matches(expected, &actual) => {}
                            Ok(actual) => report.issues.push(VerificationIssue {
                                kind: VerificationIssueKind::Mismatch,
                                plugin: "sysfs".to_string(),
                                option: pattern.clone(),
                                target: target.display().to_string(),
                                expected: expected.clone(),
                                actual: Some(actual),
                                detail: "live sysfs value differs from the resolved profile unit"
                                    .to_string(),
                            }),
                            Err(error) => report.issues.push(VerificationIssue {
                                kind: VerificationIssueKind::ReadError,
                                plugin: "sysfs".to_string(),
                                option: pattern.clone(),
                                target: target.display().to_string(),
                                expected: expected.clone(),
                                actual: None,
                                detail: error.to_string(),
                            }),
                        }
                    }
                }
                Err(error) => push_issue(
                    report,
                    VerificationIssueKind::ReadError,
                    pattern,
                    pattern,
                    expected,
                    None,
                    error.to_string(),
                ),
            }
        }
    }
}

fn remove_placeholder_contract_issues(report: &mut VerificationReport) {
    let before = report.issues.len();
    report.issues.retain(|issue| {
        !(issue.kind == VerificationIssueKind::Unsupported
            && issue.plugin == "sysfs"
            && issue.option == "plugin"
            && issue.expected == "sysfs")
    });
    report.checked = report
        .checked
        .saturating_sub(before.saturating_sub(report.issues.len()));
}

fn push_issue(
    report: &mut VerificationReport,
    kind: VerificationIssueKind,
    option: &str,
    target: &str,
    expected: &str,
    actual: Option<String>,
    detail: impl Into<String>,
) {
    report.checked += 1;
    report.issues.push(VerificationIssue {
        kind,
        plugin: "sysfs".to_string(),
        option: option.to_string(),
        target: target.to_string(),
        expected: expected.to_string(),
        actual,
        detail: detail.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_only_the_generic_sysfs_placeholder() {
        let mut report = VerificationReport {
            checked: 2,
            issues: vec![
                VerificationIssue {
                    kind: VerificationIssueKind::Unsupported,
                    plugin: "sysfs".to_string(),
                    option: "plugin".to_string(),
                    target: "sysfs".to_string(),
                    expected: "sysfs".to_string(),
                    actual: None,
                    detail: "plugin type is not implemented".to_string(),
                },
                VerificationIssue {
                    kind: VerificationIssueKind::Unsupported,
                    plugin: "cpu".to_string(),
                    option: "imaginary".to_string(),
                    target: "cpu".to_string(),
                    expected: "1".to_string(),
                    actual: None,
                    detail: "plugin option is not implemented".to_string(),
                },
            ],
        };

        remove_placeholder_contract_issues(&mut report);

        assert_eq!(report.checked, 1);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].plugin, "cpu");
    }

    #[test]
    fn missing_targets_are_waived_only_by_ignore_missing() {
        let mut report = VerificationReport::default();
        push_issue(
            &mut report,
            VerificationIssueKind::Missing,
            "/sys/devices/missing",
            "/sys/devices/missing",
            "1",
            None,
            "missing",
        );

        assert!(!report.passes(false));
        assert!(report.passes(true));
    }

    #[test]
    fn mismatches_remain_fatal_when_missing_targets_are_ignored() {
        let mut report = VerificationReport::default();
        push_issue(
            &mut report,
            VerificationIssueKind::Mismatch,
            "/sys/devices/control",
            "/sys/devices/control",
            "performance|powersave",
            Some("balanced".to_string()),
            "mismatch",
        );

        assert!(!report.passes(false));
        assert!(!report.passes(true));
    }
}
