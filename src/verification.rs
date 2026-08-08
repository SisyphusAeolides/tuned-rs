use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::{info, warn};

use crate::profile::Profile;
use crate::tuning::modifiers::{parse_assignment, AssignmentOp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationIssueKind {
    Missing,
    Mismatch,
    Unsupported,
    ReadError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationIssue {
    pub kind: VerificationIssueKind,
    pub plugin: String,
    pub option: String,
    pub target: String,
    pub expected: String,
    pub actual: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerificationReport {
    pub checked: usize,
    pub issues: Vec<VerificationIssue>,
}

impl VerificationReport {
    pub fn passes(&self, ignore_missing: bool) -> bool {
        self.issues
            .iter()
            .all(|issue| ignore_missing && issue.kind == VerificationIssueKind::Missing)
    }

    pub fn matched(&self) -> usize {
        self.checked.saturating_sub(self.issues.len())
    }

    pub fn log(&self) {
        if self.issues.is_empty() {
            info!(
                "Profile verification passed: {} target(s) matched",
                self.checked
            );
            return;
        }

        for issue in &self.issues {
            warn!(
                "Profile verification {:?}: {}.{} target={} expected='{}' actual='{}' detail={}",
                issue.kind,
                issue.plugin,
                issue.option,
                issue.target,
                issue.expected,
                issue.actual.as_deref().unwrap_or("<unavailable>"),
                issue.detail
            );
        }
        warn!(
            "Profile verification failed: {} of {} target(s) matched",
            self.matched(),
            self.checked
        );
    }
}

pub fn verify_profile(profile: &Profile) -> VerificationReport {
    Verifier::system().verify(profile)
}

#[derive(Debug, Clone)]
struct Verifier {
    root: Option<PathBuf>,
}

impl Verifier {
    fn system() -> Self {
        Self {
            root: std::env::var_os("TUNED_RS_ROOT").map(PathBuf::from),
        }
    }

    #[cfg(test)]
    fn rooted(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    fn path(&self, absolute: &str) -> PathBuf {
        match &self.root {
            Some(root) => root.join(absolute.trim_start_matches('/')),
            None => PathBuf::from(absolute),
        }
    }

    fn verify(&self, profile: &Profile) -> VerificationReport {
        let mut report = VerificationReport::default();
        self.verify_cpu(profile, &mut report);
        self.verify_sysctl(profile, &mut report);
        self.verify_vm(profile, &mut report);
        self.verify_disk(profile, &mut report);
        self.verify_acpi(profile, &mut report);
        self.verify_network(profile, &mut report);
        self.verify_gpu(profile, &mut report);
        self.verify_storage(profile, &mut report);
        self.verify_thermal(profile, &mut report);
        self.verify_battery(profile, &mut report);
        self.verify_hermes(profile, &mut report);
        report
    }

    fn verify_cpu(&self, profile: &Profile, report: &mut VerificationReport) {
        let policies = matching_children(&self.path("/sys/devices/system/cpu/cpufreq"), |name| {
            name.strip_prefix("policy").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
            })
        });

        for (option, expected, leaf) in [
            (
                "governor",
                profile.cpu.governor.as_deref(),
                "scaling_governor",
            ),
            (
                "energy_performance_preference",
                profile.cpu.energy_performance_preference.as_deref(),
                "energy_performance_preference",
            ),
        ] {
            let Some(expected) = expected else {
                continue;
            };
            if policies.is_empty() {
                report.missing(
                    "cpu",
                    option,
                    self.path("/sys/devices/system/cpu/cpufreq"),
                    expected,
                );
                continue;
            }
            for policy in &policies {
                self.check_path(report, "cpu", option, policy.join(leaf), expected);
            }
        }
    }

    fn verify_sysctl(&self, profile: &Profile, report: &mut VerificationReport) {
        let mut entries = profile.sysctl.iter().collect::<Vec<_>>();
        entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
        for (key, expected) in entries {
            if !valid_sysctl_key(key) {
                report.unsupported("sysctl", key, key, expected, "invalid sysctl key");
                continue;
            }
            let path = self.path(&format!("/proc/sys/{}", key.replace('.', "/")));
            self.check_path(report, "sysctl", key, path, expected);
        }
    }

    fn verify_vm(&self, profile: &Profile, report: &mut VerificationReport) {
        for (option, expected) in [
            ("dirty_ratio", profile.vm.dirty_ratio.as_deref()),
            (
                "dirty_background_ratio",
                profile.vm.dirty_background_ratio.as_deref(),
            ),
            ("dirty_bytes", profile.vm.dirty_bytes.as_deref()),
            (
                "dirty_background_bytes",
                profile.vm.dirty_background_bytes.as_deref(),
            ),
        ] {
            let Some(expected) = expected else {
                continue;
            };
            let (effective_option, expected) =
                crate::tuning::vm::effective_option_value(option, expected);
            self.check_path(
                report,
                "vm",
                option,
                self.path(&format!("/proc/sys/vm/{effective_option}")),
                &expected,
            );
        }

        if let Some(expected) = profile.vm.transparent_hugepages.as_deref() {
            match self.thp_directory() {
                Some(directory) => self.check_path(
                    report,
                    "vm",
                    "transparent_hugepages",
                    directory.join("enabled"),
                    expected,
                ),
                None => report.missing(
                    "vm",
                    "transparent_hugepages",
                    self.path("/sys/kernel/mm/transparent_hugepage/enabled"),
                    expected,
                ),
            }
        }

        if let Some(expected) = profile.vm.transparent_hugepage_defrag.as_deref() {
            match self.thp_directory() {
                Some(directory) => self.check_path(
                    report,
                    "vm",
                    "transparent_hugepage.defrag",
                    directory.join("defrag"),
                    expected,
                ),
                None => report.missing(
                    "vm",
                    "transparent_hugepage.defrag",
                    self.path("/sys/kernel/mm/transparent_hugepage/defrag"),
                    expected,
                ),
            }
        }
    }

    fn verify_disk(&self, profile: &Profile, report: &mut VerificationReport) {
        let options = [
            ("elevator", profile.disk.elevator.as_deref()),
            ("readahead", profile.disk.readahead.as_deref()),
        ];
        if options.iter().all(|(_, value)| value.is_none()) {
            return;
        }

        let devices = match profile.disk.devices.as_deref() {
            Some(raw) => raw
                .split([',', ' '])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>(),
            None => matching_child_names(&self.path("/sys/block"), tunable_block_device),
        };

        if devices.is_empty() {
            for (option, expected) in options {
                if let Some(expected) = expected {
                    report.missing("disk", option, self.path("/sys/block"), expected);
                }
            }
            return;
        }

        for device in devices {
            if !valid_device_name(&device) {
                for (option, expected) in options {
                    if let Some(expected) = expected {
                        report.unsupported(
                            "disk",
                            option,
                            &device,
                            expected,
                            "invalid block device name",
                        );
                    }
                }
                continue;
            }
            if let Some(expected) = profile.disk.elevator.as_deref() {
                self.check_path(
                    report,
                    "disk",
                    "elevator",
                    self.path(&format!("/sys/block/{device}/queue/scheduler")),
                    expected,
                );
            }
            if let Some(raw) = profile.disk.readahead.as_deref() {
                match normalized_readahead(raw) {
                    Ok(expected) => self.check_path(
                        report,
                        "disk",
                        "readahead",
                        self.path(&format!("/sys/block/{device}/queue/read_ahead_kb")),
                        &expected,
                    ),
                    Err(detail) => report.unsupported("disk", "readahead", &device, raw, detail),
                }
            }
        }
    }

    fn verify_acpi(&self, profile: &Profile, report: &mut VerificationReport) {
        if let Some(expected) = profile.acpi.platform_profile.as_deref() {
            self.check_path(
                report,
                "acpi",
                "platform_profile",
                self.path("/sys/firmware/acpi/platform_profile"),
                expected,
            );
        }
    }

    fn verify_network(&self, profile: &Profile, report: &mut VerificationReport) {
        for (option, expected, relative) in [
            (
                "tcp_congestion_control",
                profile.network.tcp_congestion_control.as_deref(),
                "ipv4/tcp_congestion_control",
            ),
            (
                "tcp_window_scaling",
                profile.network.tcp_window_scaling.as_deref(),
                "ipv4/tcp_window_scaling",
            ),
            (
                "tcp_timestamps",
                profile.network.tcp_timestamps.as_deref(),
                "ipv4/tcp_timestamps",
            ),
            (
                "tcp_sack",
                profile.network.tcp_sack.as_deref(),
                "ipv4/tcp_sack",
            ),
            (
                "tcp_fastopen",
                profile.network.tcp_fastopen.as_deref(),
                "ipv4/tcp_fastopen",
            ),
        ] {
            if let Some(expected) = expected {
                self.check_path(
                    report,
                    "network",
                    option,
                    self.path(&format!("/proc/sys/net/{relative}")),
                    expected,
                );
            }
        }
    }

    fn verify_gpu(&self, profile: &Profile, report: &mut VerificationReport) {
        for (option, expected) in &profile.gpu {
            match option.as_str() {
                "amd_power_profile" | "amd_power_dpm_force_performance_level" => {
                    let targets = matching_children(&self.path("/sys/class/drm"), |name| {
                        name.starts_with("card") && !name.contains('-')
                    })
                    .into_iter()
                    .map(|card| card.join("device/power_dpm_force_performance_level"))
                    .filter(|path| path.exists())
                    .collect::<Vec<_>>();
                    match targets.first() {
                        Some(target) => {
                            self.check_path(report, "gpu", option, target.clone(), expected)
                        }
                        None => {
                            report.missing("gpu", option, self.path("/sys/class/drm"), expected)
                        }
                    }
                }
                "nvidia_power_limit" => {
                    self.verify_nvidia_query(report, option, "power.limit", expected, true)
                }
                "nvidia_persistence_mode" => {
                    let expected = match expected.as_str() {
                        "on" | "1" | "true" => "Enabled",
                        "off" | "0" | "false" => "Disabled",
                        _ => {
                            report.unsupported(
                                "gpu",
                                option,
                                "nvidia-smi",
                                expected,
                                "invalid persistence mode",
                            );
                            continue;
                        }
                    };
                    self.verify_nvidia_query(report, option, "persistence_mode", expected, false);
                }
                "nvidia_graphics_clock" | "nvidia_memory_clock" => report.unsupported(
                    "gpu",
                    option,
                    "nvidia-smi",
                    expected,
                    "the driver does not expose a stable readback for locked application clocks",
                ),
                other => report.unsupported("gpu", other, "gpu", expected, "unknown GPU option"),
            }
        }
    }

    fn verify_nvidia_query(
        &self,
        report: &mut VerificationReport,
        option: &str,
        field: &str,
        expected: &str,
        numeric: bool,
    ) {
        report.checked += 1;
        let output = Command::new("nvidia-smi")
            .arg(format!("--query-gpu={field}"))
            .arg("--format=csv,noheader,nounits")
            .output();
        let output = match output {
            Ok(output) if output.status.success() => output,
            Ok(output) => {
                report.issues.push(VerificationIssue {
                    kind: VerificationIssueKind::ReadError,
                    plugin: "gpu".to_string(),
                    option: option.to_string(),
                    target: "nvidia-smi".to_string(),
                    expected: expected.to_string(),
                    actual: None,
                    detail: format!("nvidia-smi exited with {}", output.status),
                });
                return;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                report.missing("gpu", option, PathBuf::from("nvidia-smi"), expected);
                return;
            }
            Err(error) => {
                report.read_error(
                    "gpu",
                    option,
                    PathBuf::from("nvidia-smi"),
                    expected,
                    error.to_string(),
                );
                return;
            }
        };

        let values = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if values.is_empty() {
            report.missing("gpu", option, PathBuf::from("nvidia-smi"), expected);
            return;
        }
        for (index, actual) in values.iter().enumerate() {
            let matches = if numeric {
                numeric_equivalent(expected, actual)
            } else {
                normalize(expected) == normalize(actual)
            };
            if !matches {
                report.issues.push(VerificationIssue {
                    kind: VerificationIssueKind::Mismatch,
                    plugin: "gpu".to_string(),
                    option: option.to_string(),
                    target: format!("nvidia-smi:gpu{index}"),
                    expected: expected.to_string(),
                    actual: Some(actual.clone()),
                    detail: "live NVIDIA state differs from the resolved profile".to_string(),
                });
            }
        }
    }

    fn verify_storage(&self, profile: &Profile, report: &mut VerificationReport) {
        for (option, expected) in &profile.storage {
            match option.as_str() {
                "nvme_apst" => {
                    let targets = matching_children(&self.path("/sys/class/nvme"), |name| {
                        name.starts_with("nvme")
                    })
                    .into_iter()
                    .map(|path| path.join("power/pm_qos_latency_tolerance_us"))
                    .filter(|path| path.exists())
                    .collect::<Vec<_>>();
                    self.check_targets(report, "storage", option, expected, targets);
                }
                "io_scheduler" => {
                    let targets =
                        matching_children(&self.path("/sys/block"), tunable_storage_device)
                            .into_iter()
                            .map(|path| path.join("queue/scheduler"))
                            .filter(|path| path.exists())
                            .collect::<Vec<_>>();
                    self.check_targets(report, "storage", option, expected, targets);
                }
                "nr_requests" => {
                    let targets =
                        matching_children(&self.path("/sys/block"), tunable_storage_device)
                            .into_iter()
                            .map(|path| path.join("queue/nr_requests"))
                            .filter(|path| path.exists())
                            .collect::<Vec<_>>();
                    self.check_targets(report, "storage", option, expected, targets);
                }
                "ssd_trim" => report.unsupported(
                    "storage",
                    option,
                    "fstrim.timer",
                    expected,
                    "SSD TRIM application is not yet transactional",
                ),
                other => report.unsupported(
                    "storage",
                    other,
                    "storage",
                    expected,
                    "unknown storage option",
                ),
            }
        }
    }

    fn verify_thermal(&self, profile: &Profile, report: &mut VerificationReport) {
        for (option, expected) in &profile.thermal {
            match option.as_str() {
                "cpu_temp_limit" | "trip_point" => {
                    let target = matching_children(&self.path("/sys/class/thermal"), |name| {
                        name.starts_with("thermal_zone")
                    })
                    .into_iter()
                    .find(|zone| {
                        fs::read_to_string(zone.join("type"))
                            .map(|kind| kind.contains("cpu") || kind.contains("x86_pkg_temp"))
                            .unwrap_or(false)
                            && zone.join("trip_point_0_temp").exists()
                    })
                    .map(|zone| zone.join("trip_point_0_temp"));
                    let expected_millidegrees = expected
                        .parse::<u32>()
                        .map(|value| value.saturating_mul(1000).to_string())
                        .unwrap_or_else(|_| "85000".to_string());
                    match target {
                        Some(target) => self.check_path(
                            report,
                            "thermal",
                            option,
                            target,
                            &expected_millidegrees,
                        ),
                        None => report.missing(
                            "thermal",
                            option,
                            self.path("/sys/class/thermal"),
                            &expected_millidegrees,
                        ),
                    }
                }
                "fan_control" => {
                    let target = matching_children(&self.path("/sys/class/hwmon"), |_| true)
                        .into_iter()
                        .map(|path| path.join("pwm1_enable"))
                        .find(|path| path.exists());
                    let mode = match expected.as_str() {
                        "auto" | "automatic" => "2",
                        "manual" | "full" => "1",
                        _ => {
                            report.unsupported(
                                "thermal",
                                option,
                                "pwm1_enable",
                                expected,
                                "unknown fan control mode",
                            );
                            continue;
                        }
                    };
                    match target {
                        Some(target) => self.check_path(report, "thermal", option, target, mode),
                        None => {
                            report.missing("thermal", option, self.path("/sys/class/hwmon"), mode)
                        }
                    }
                }
                "thermal_policy" => self.check_path(
                    report,
                    "thermal",
                    option,
                    self.path("/sys/devices/virtual/thermal/thermal_zone0/policy"),
                    expected,
                ),
                other => report.unsupported(
                    "thermal",
                    other,
                    "thermal",
                    expected,
                    "unknown thermal option",
                ),
            }
        }
    }

    fn verify_battery(&self, profile: &Profile, report: &mut VerificationReport) {
        for (option, expected) in &profile.battery {
            match option.as_str() {
                "charge_start_threshold" | "charge_stop_threshold" | "battery_care_limit" => {
                    let leaf = if option == "charge_start_threshold" {
                        "charge_control_start_threshold"
                    } else {
                        "charge_control_end_threshold"
                    };
                    let target = matching_children(&self.path("/sys/class/power_supply"), |name| {
                        name.starts_with("BAT")
                    })
                    .into_iter()
                    .map(|path| path.join(leaf))
                    .find(|path| path.exists());
                    match target {
                        Some(target) => {
                            self.check_path(report, "battery", option, target, expected)
                        }
                        None => report.missing(
                            "battery",
                            option,
                            self.path("/sys/class/power_supply"),
                            expected,
                        ),
                    }
                }
                "conservation_mode" => {
                    let mode = match expected.as_str() {
                        "on" | "1" | "true" => "1",
                        "off" | "0" | "false" => "0",
                        _ => {
                            report.unsupported(
                                "battery",
                                option,
                                "conservation_mode",
                                expected,
                                "invalid conservation mode",
                            );
                            continue;
                        }
                    };
                    self.check_path(
                        report,
                        "battery",
                        option,
                        self.path(
                            "/sys/bus/platform/drivers/ideapad_acpi/VPC2004:00/conservation_mode",
                        ),
                        mode,
                    );
                }
                other => report.unsupported(
                    "battery",
                    other,
                    "battery",
                    expected,
                    "unknown battery option",
                ),
            }
        }
    }

    fn verify_hermes(&self, profile: &Profile, report: &mut VerificationReport) {
        for (option, expected) in &profile.hermes {
            match option.as_str() {
                "cmd_ring_size"
                | "rsp_ring_size"
                | "ring_overflow_threshold"
                | "ring_poll_interval"
                | "runtime_pm_enabled"
                | "idle_timeout_ms"
                | "autosuspend_delay"
                | "debug_level"
                | "error_recovery_mode"
                | "firmware_validation" => self.check_path(
                    report,
                    "hermes",
                    option,
                    self.path(&format!("/sys/module/hermes/parameters/{option}")),
                    expected,
                ),
                "display_heads" => {
                    let maximum = expected.parse::<usize>().unwrap_or(4).min(4);
                    let devices = matching_children(&self.path("/sys/class/hermes"), |_| true);
                    if devices.is_empty() {
                        report.missing("hermes", option, self.path("/sys/class/hermes"), expected);
                        continue;
                    }
                    let mut found = false;
                    for device in devices {
                        for head in 0..4 {
                            let target = device.join(format!("display/head{head}/enabled"));
                            if !target.exists() {
                                continue;
                            }
                            found = true;
                            let enabled = if head < maximum { "1" } else { "0" };
                            self.check_path(report, "hermes", option, target, enabled);
                        }
                    }
                    if !found {
                        report.missing("hermes", option, self.path("/sys/class/hermes"), expected);
                    }
                }
                "gsp_power_mode" => {
                    let (idle_timeout, runtime_pm) = match expected.as_str() {
                        "performance" => ("0", "0"),
                        "balanced" => ("5000", "1"),
                        "powersave" => ("1000", "1"),
                        _ => {
                            report.unsupported(
                                "hermes",
                                option,
                                "gsp_power_mode",
                                expected,
                                "unknown GSP power mode",
                            );
                            continue;
                        }
                    };
                    self.check_path(
                        report,
                        "hermes",
                        option,
                        self.path("/sys/module/hermes/parameters/idle_timeout_ms"),
                        idle_timeout,
                    );
                    self.check_path(
                        report,
                        "hermes",
                        option,
                        self.path("/sys/module/hermes/parameters/runtime_pm_enabled"),
                        runtime_pm,
                    );
                }
                other => {
                    report.unsupported("hermes", other, "hermes", expected, "unknown Hermes option")
                }
            }
        }
    }

    fn check_targets(
        &self,
        report: &mut VerificationReport,
        plugin: &str,
        option: &str,
        expected: &str,
        targets: Vec<PathBuf>,
    ) {
        if targets.is_empty() {
            report.missing(plugin, option, self.path("/sys"), expected);
            return;
        }
        for target in targets {
            self.check_path(report, plugin, option, target, expected);
        }
    }

    fn check_path(
        &self,
        report: &mut VerificationReport,
        plugin: &str,
        option: &str,
        target: PathBuf,
        expected: &str,
    ) {
        report.checked += 1;
        match fs::read_to_string(&target) {
            Ok(actual) => {
                let actual = actual.trim().to_string();
                if !matches_expected(expected, &actual) {
                    report.issues.push(VerificationIssue {
                        kind: VerificationIssueKind::Mismatch,
                        plugin: plugin.to_string(),
                        option: option.to_string(),
                        target: target.display().to_string(),
                        expected: expected.to_string(),
                        actual: Some(actual),
                        detail: "live value differs from the resolved profile".to_string(),
                    });
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                report.issues.push(VerificationIssue {
                    kind: VerificationIssueKind::Missing,
                    plugin: plugin.to_string(),
                    option: option.to_string(),
                    target: target.display().to_string(),
                    expected: expected.to_string(),
                    actual: None,
                    detail: "target is not available on this system".to_string(),
                });
            }
            Err(error) => report.issues.push(VerificationIssue {
                kind: VerificationIssueKind::ReadError,
                plugin: plugin.to_string(),
                option: option.to_string(),
                target: target.display().to_string(),
                expected: expected.to_string(),
                actual: None,
                detail: error.to_string(),
            }),
        }
    }

    fn thp_directory(&self) -> Option<PathBuf> {
        [
            self.path("/sys/kernel/mm/transparent_hugepage"),
            self.path("/sys/kernel/mm/redhat_transparent_hugepage"),
        ]
        .into_iter()
        .find(|path| path.is_dir())
    }
}

impl VerificationReport {
    fn missing(&mut self, plugin: &str, option: &str, target: PathBuf, expected: &str) {
        self.checked += 1;
        self.issues.push(VerificationIssue {
            kind: VerificationIssueKind::Missing,
            plugin: plugin.to_string(),
            option: option.to_string(),
            target: target.display().to_string(),
            expected: expected.to_string(),
            actual: None,
            detail: "target is not available on this system".to_string(),
        });
    }

    fn unsupported(
        &mut self,
        plugin: &str,
        option: &str,
        target: &str,
        expected: &str,
        detail: impl Into<String>,
    ) {
        self.checked += 1;
        self.issues.push(VerificationIssue {
            kind: VerificationIssueKind::Unsupported,
            plugin: plugin.to_string(),
            option: option.to_string(),
            target: target.to_string(),
            expected: expected.to_string(),
            actual: None,
            detail: detail.into(),
        });
    }

    fn read_error(
        &mut self,
        plugin: &str,
        option: &str,
        target: PathBuf,
        expected: &str,
        detail: impl Into<String>,
    ) {
        self.checked += 1;
        self.issues.push(VerificationIssue {
            kind: VerificationIssueKind::ReadError,
            plugin: plugin.to_string(),
            option: option.to_string(),
            target: target.display().to_string(),
            expected: expected.to_string(),
            actual: None,
            detail: detail.into(),
        });
    }
}

fn matches_expected(expected: &str, actual: &str) -> bool {
    let assignment = parse_assignment(expected);
    let actual = active_value(actual);
    match assignment.op {
        AssignmentOp::Set => assignment
            .target
            .split('|')
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
            .any(|candidate| normalize(candidate) == normalize(actual)),
        AssignmentOp::Greater | AssignmentOp::GreaterEqual => {
            compare_numeric(&assignment.target, actual, |actual, target| {
                actual >= target
            })
        }
        AssignmentOp::Less | AssignmentOp::LessEqual => {
            compare_numeric(&assignment.target, actual, |actual, target| {
                actual <= target
            })
        }
    }
}

fn compare_numeric(target: &str, actual: &str, compare: impl Fn(i64, i64) -> bool) -> bool {
    match (target.trim().parse::<i64>(), actual.trim().parse::<i64>()) {
        (Ok(target), Ok(actual)) => compare(actual, target),
        _ => normalize(target) == normalize(actual),
    }
}

fn numeric_equivalent(expected: &str, actual: &str) -> bool {
    match (expected.trim().parse::<f64>(), actual.trim().parse::<f64>()) {
        (Ok(expected), Ok(actual)) => (expected - actual).abs() < 0.01,
        _ => normalize(expected) == normalize(actual),
    }
}

fn active_value(raw: &str) -> &str {
    let Some(start) = raw.find('[') else {
        return raw.trim();
    };
    let Some(relative_end) = raw[start + 1..].find(']') else {
        return raw.trim();
    };
    raw[start + 1..start + 1 + relative_end].trim()
}

fn normalize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalized_readahead(raw: &str) -> Result<String, String> {
    let assignment = parse_assignment(raw);
    let mut parts = assignment.target.split_whitespace();
    let value = parts
        .next()
        .ok_or_else(|| "missing readahead value".to_string())?
        .parse::<i64>()
        .map_err(|error| format!("invalid readahead value: {error}"))?;
    let value = match parts.next() {
        None => value,
        Some("s") => value / 2,
        Some(unit) => return Err(format!("unsupported readahead unit '{unit}'")),
    };
    if parts.next().is_some() {
        return Err("too many readahead fields".to_string());
    }
    let prefix = match assignment.op {
        AssignmentOp::Set => "",
        AssignmentOp::Greater => ">",
        AssignmentOp::GreaterEqual => ">=",
        AssignmentOp::Less => "<",
        AssignmentOp::LessEqual => "<=",
    };
    Ok(format!("{prefix}{value}"))
}

fn matching_children(base: &Path, predicate: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut children = fs::read_dir(base)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            predicate(&name).then(|| entry.path())
        })
        .collect::<Vec<_>>();
    children.sort_unstable();
    children
}

fn matching_child_names(base: &Path, predicate: impl Fn(&str) -> bool) -> Vec<String> {
    matching_children(base, predicate)
        .into_iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect()
}

fn valid_sysctl_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 256
        && !key.starts_with('.')
        && !key.ends_with('.')
        && !key.contains("..")
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn valid_device_name(device: &str) -> bool {
    !device.is_empty()
        && !device.contains('/')
        && device
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

fn tunable_block_device(name: &str) -> bool {
    !(name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("fd")
        || name.starts_with("dm-")
        || name.starts_with("sr"))
}

fn tunable_storage_device(name: &str) -> bool {
    !(name.starts_with("loop") || name.starts_with("ram"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, absolute: &str, value: &str) {
        let path = root.join(absolute.trim_start_matches('/'));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }

    #[test]
    fn assignment_and_active_choice_comparisons_are_semantic() {
        assert!(matches_expected(
            "schedutil|performance",
            "[performance] powersave"
        ));
        assert!(matches_expected(">=4096", "8192"));
        assert!(!matches_expected(">=4096", "2048"));
        assert!(matches_expected("<100", "80"));
        assert!(!matches_expected("<100", "120"));
    }

    #[test]
    fn ignore_missing_never_hides_a_mismatch() {
        let root = TempDir::new().unwrap();
        write(root.path(), "/proc/sys/vm/swappiness", "60\n");
        let mut profile = Profile::default();
        profile
            .sysctl
            .insert("vm.swappiness".to_string(), "10".to_string());

        let report = Verifier::rooted(root.path()).verify(&profile);
        assert_eq!(report.issues[0].kind, VerificationIssueKind::Mismatch);
        assert!(!report.passes(false));
        assert!(!report.passes(true));

        fs::remove_file(root.path().join("proc/sys/vm/swappiness")).unwrap();
        let report = Verifier::rooted(root.path()).verify(&profile);
        assert_eq!(report.issues[0].kind, VerificationIssueKind::Missing);
        assert!(!report.passes(false));
        assert!(report.passes(true));
    }

    #[test]
    fn verifies_a_resolved_profile_against_a_fake_kernel_tree() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "/sys/devices/system/cpu/cpufreq/policy0/scaling_governor",
            "performance\n",
        );
        write(root.path(), "/proc/sys/vm/swappiness", "10\n");
        write(
            root.path(),
            "/sys/kernel/mm/transparent_hugepage/enabled",
            "always [madvise] never\n",
        );
        write(
            root.path(),
            "/sys/block/nvme0n1/queue/scheduler",
            "none [mq-deadline] kyber\n",
        );
        write(
            root.path(),
            "/sys/block/nvme0n1/queue/read_ahead_kb",
            "4096\n",
        );
        write(
            root.path(),
            "/sys/firmware/acpi/platform_profile",
            "balanced\n",
        );
        write(root.path(), "/proc/sys/net/ipv4/tcp_window_scaling", "1\n");

        let mut profile = Profile::default();
        profile.cpu.governor = Some("schedutil|performance".to_string());
        profile
            .sysctl
            .insert("vm.swappiness".to_string(), "10".to_string());
        profile.vm.transparent_hugepages = Some("madvise".to_string());
        profile.disk.devices = Some("nvme0n1".to_string());
        profile.disk.elevator = Some("mq-deadline".to_string());
        profile.disk.readahead = Some("=>4096".to_string());
        profile.acpi.platform_profile = Some("performance|balanced".to_string());
        profile.network.tcp_window_scaling = Some("1".to_string());

        let report = Verifier::rooted(root.path()).verify(&profile);
        assert!(report.issues.is_empty(), "{:#?}", report.issues);
        assert_eq!(report.checked, 7);
        assert!(report.passes(false));
    }

    #[test]
    fn unsupported_options_fail_closed() {
        let mut profile = Profile::default();
        profile
            .storage
            .push(("ssd_trim".to_string(), "on".to_string()));
        let report = Verifier::rooted("/nonexistent").verify(&profile);
        assert_eq!(report.issues[0].kind, VerificationIssueKind::Unsupported);
        assert!(!report.passes(true));
    }
}
