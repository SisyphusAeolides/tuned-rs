use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::config;
use crate::plugins;
use crate::profile::Profile;
use crate::profile_runtime;
use crate::profile_units::{option_value, ProfileUnit};
use crate::tuning;
use crate::verification::{VerificationIssue, VerificationIssueKind, VerificationReport};

pub fn augment(profile: &Profile, report: &mut VerificationReport) {
    let units = match profile_runtime::active_units(profile) {
        Ok(units) => units,
        Err(error) => {
            issue(
                report,
                VerificationIssueKind::ReadError,
                "profile",
                "conditions",
                "runtime profile",
                "active units",
                None,
                error.to_string(),
            );
            return;
        }
    };

    for unit in units {
        let (unit, devices) = match tuning::resolve_unit_device_controls(&unit) {
            Ok(resolved) => resolved,
            Err(error) => {
                issue(
                    report,
                    VerificationIssueKind::Unsupported,
                    &unit.plugin_type,
                    "device-controls",
                    &unit.name,
                    "valid device controls",
                    None,
                    error.to_string(),
                );
                continue;
            }
        };
        if !verify_contract(&unit, report) {
            continue;
        }
        for (phase, hook) in [
            ("script_pre", unit.script_pre.as_deref()),
            ("script_post", unit.script_post.as_deref()),
        ] {
            if hook.is_some_and(|path| {
                !tuning::script::verify_device_script(Path::new(path), &devices, true)
            }) {
                issue(
                    report,
                    VerificationIssueKind::Mismatch,
                    &unit.plugin_type,
                    phase,
                    &unit.name,
                    "successful device-hook verification",
                    None,
                    "device hook verification failed",
                );
            }
        }
        match unit.plugin_type.as_str() {
            "modules" => verify_modules(&unit, report),
            "cpu" => verify_cpu(&unit, report),
            "sysctl" => verify_sysctl(&unit, report),
            "vm" => verify_vm(&unit, report),
            "disk" => verify_disk(&unit, report),
            "acpi" => verify_acpi(&unit, report),
            "net" | "network" => verify_network(&unit, report),
            "audio" => verify_audio(&unit, report),
            "video" => verify_video(&unit, report),
            "scsi_host" => verify_scsi_host(&unit, report),
            "selinux" => verify_selinux(&unit, report),
            "usb" => verify_usb(&unit, report),
            "systemd" => verify_systemd(&unit, report),
            "uncore" => verify_uncore(&unit, report),
            "irqbalance" => verify_irqbalance(&unit, report),
            "irq" => verify_irq(&unit, report),
            "rtentsk" => verify_rtentsk(&unit, report),
            "scheduler" => verify_scheduler(&unit, report),
            "eeepc_she" => verify_eeepc_she(&unit, report),
            "bootloader" => verify_bootloader(&unit, report),
            "script" => verify_script(&unit, report),
            "service" => verify_service(&unit, report),
            "mounts" => verify_mounts(&unit, report),
            "gpu" | "storage" | "thermal" | "battery" | "hermes" => {
                if is_conditional(&unit) {
                    issue(
                        report,
                        VerificationIssueKind::Unsupported,
                        &unit.plugin_type,
                        "conditional-unit-verification",
                        &unit.name,
                        "verified conditional unit",
                        None,
                        "detailed conditional verification is not implemented for this plugin",
                    );
                }
            }
            _ => {}
        }
    }
}

fn verify_contract(unit: &ProfileUnit, report: &mut VerificationReport) -> bool {
    let mut valid = true;
    if unit.devices != "*"
        && !matches!(
            unit.plugin_type.as_str(),
            "audio"
                | "cpu"
                | "disk"
                | "scsi_host"
                | "usb"
                | "uncore"
                | "video"
                | "net"
                | "network"
                | "irq"
                | "mounts"
        )
    {
        issue(
            report,
            VerificationIssueKind::Unsupported,
            &unit.plugin_type,
            "devices",
            &unit.name,
            &unit.devices,
            None,
            "device selection is not implemented for this plugin",
        );
        valid = false;
    }

    let Some(descriptor) = plugins::descriptor(&unit.plugin_type) else {
        issue(
            report,
            VerificationIssueKind::Unsupported,
            &unit.plugin_type,
            "plugin",
            &unit.name,
            &unit.plugin_type,
            None,
            "plugin type is not implemented",
        );
        return false;
    };
    if !matches!(unit.plugin_type.as_str(), "modules" | "sysctl") {
        for (option, expected) in &unit.options {
            if unit.plugin_type == "scheduler"
                && (option.starts_with("group.") || option.starts_with("cgroup."))
            {
                continue;
            }
            if unit.plugin_type == "bootloader" && option.starts_with("cmdline") {
                continue;
            }
            if !descriptor
                .options
                .iter()
                .any(|supported| supported.name == option)
            {
                issue(
                    report,
                    VerificationIssueKind::Unsupported,
                    &unit.plugin_type,
                    option,
                    &unit.name,
                    expected,
                    None,
                    "plugin option is not implemented",
                );
                valid = false;
            }
        }
    }
    valid
}

fn verify_selinux(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::selinux::verify_options(&unit.options, true) {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "selinux",
            "avc_cache_threshold",
            &unit.name,
            unit.option("avc_cache_threshold").unwrap_or_default(),
            None,
            "SELinux AVC cache threshold does not match",
        );
    }
}

fn verify_usb(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::usb::verify_options(&unit.devices, &unit.options, true) {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "usb",
            "autosuspend",
            &unit.name,
            unit.option("autosuspend").unwrap_or_default(),
            None,
            "USB autosuspend settings do not match",
        );
    }
}

fn verify_systemd(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::systemd::verify_options(&unit.options, true) {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "systemd",
            "cpu_affinity",
            &unit.name,
            unit.option("cpu_affinity").unwrap_or_default(),
            None,
            "systemd CPUAffinity does not match",
        );
    }
}

fn verify_uncore(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::uncore::verify_options(&unit.devices, &unit.options, true) {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "uncore",
            "frequency",
            &unit.name,
            "configured uncore frequency limits",
            None,
            "Intel uncore frequency limits do not match",
        );
    }
}

fn verify_irqbalance(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::irqbalance::verify_options(&unit.options, true) {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "irqbalance",
            "banned_cpus",
            &unit.name,
            unit.option("banned_cpus").unwrap_or_default(),
            None,
            "irqbalance banned CPU list does not match",
        );
    }
}

fn verify_irq(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::irq::verify_options(&unit.devices, &unit.options, true) {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "irq",
            "affinity",
            &unit.name,
            option_value(&unit.options, "affinity").unwrap_or_default(),
            None,
            "one or more selected IRQ affinities do not match",
        );
    }
}

fn verify_rtentsk(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::rtentsk::verify() {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "rtentsk",
            "socket",
            &unit.name,
            "open timestamping socket",
            None,
            "RTENTSK timestamping socket is not active",
        );
    }
}

fn verify_scheduler(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::scheduler::verify_options(&unit.options, true) {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "scheduler",
            "kernel-knobs",
            &unit.name,
            "configured scheduler values",
            None,
            "kernel scheduler tunables do not match",
        );
    }
}

fn verify_eeepc_she(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::eeepc_she::verify() {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "eeepc_she",
            "runtime-monitor",
            &unit.name,
            "active load monitor",
            None,
            "EeePC SHE runtime monitor is not active",
        );
    }
}

fn verify_bootloader(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::bootloader::verify_options(&unit.options, true) {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "bootloader",
            "cmdline",
            &unit.name,
            "configured kernel arguments",
            None,
            "active kernel command line does not include the profile arguments",
        );
    }
}

fn verify_cpu(unit: &ProfileUnit, report: &mut VerificationReport) {
    let all_cpus = matching_children(&rooted("/sys/devices/system/cpu"), |name| {
        name.strip_prefix("cpu")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
    });
    let selected_names = crate::device_matcher::filter_names(
        &unit.devices,
        all_cpus.iter().filter_map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        }),
    );
    let selected_names = selected_names
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let cpus = all_cpus
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| selected_names.contains(name))
        })
        .collect::<Vec<_>>();
    let policy_root = rooted("/sys/devices/system/cpu/cpufreq");
    let policies = cpus
        .iter()
        .filter_map(|path| {
            let cpu = path.file_name()?.to_str()?;
            let id = cpu.strip_prefix("cpu")?;
            let policy = policy_root.join(format!("policy{id}"));
            policy.is_dir().then_some(policy)
        })
        .collect::<Vec<_>>();

    for (option, expected) in &unit.options {
        match option.as_str() {
            "governor" => check_paths(
                report,
                "cpu",
                option,
                expected,
                policies
                    .iter()
                    .map(|path| path.join("scaling_governor"))
                    .filter(|path| path.is_file())
                    .collect(),
                ValueMode::Choice,
            ),
            "energy_performance_preference" => check_paths(
                report,
                "cpu",
                option,
                expected,
                policies
                    .iter()
                    .map(|path| path.join("energy_performance_preference"))
                    .filter(|path| path.is_file())
                    .collect(),
                ValueMode::Choice,
            ),
            "energy_perf_bias" => check_paths_with(
                report,
                "cpu",
                option,
                expected,
                cpus.iter()
                    .map(|path| path.join("power/energy_perf_bias"))
                    .filter(|path| path.is_file())
                    .collect(),
                energy_bias_matches,
            ),
            "min_perf_pct" | "max_perf_pct" => {
                let targets = ["intel_pstate", "amd_pstate"]
                    .into_iter()
                    .map(|driver| rooted(&format!("/sys/devices/system/cpu/{driver}/{option}")))
                    .filter(|path| path.is_file())
                    .collect();
                check_paths(report, "cpu", option, expected, targets, ValueMode::Exact);
            }
            "boost" => verify_boost(report, option, expected, &policies),
            "no_turbo" => verify_no_turbo(report, option, expected),
            "pm_qos_resume_latency_us" => check_paths(
                report,
                "cpu",
                option,
                expected,
                cpus.iter()
                    .map(|path| path.join("power/pm_qos_resume_latency_us"))
                    .filter(|path| path.is_file())
                    .collect(),
                ValueMode::Exact,
            ),
            "sampling_down_factor" => {
                let mut targets = Vec::new();
                for policy in &policies {
                    if let Ok(governor) = fs::read_to_string(policy.join("scaling_governor")) {
                        let target = rooted(&format!(
                            "/sys/devices/system/cpu/cpufreq/{}/sampling_down_factor",
                            governor.trim()
                        ));
                        if target.is_file() && !targets.contains(&target) {
                            targets.push(target);
                        }
                    }
                }
                check_paths(report, "cpu", option, expected, targets, ValueMode::Exact);
            }
            "force_latency" => {
                if !tuning::cpu::verify_force_latency(expected) {
                    issue(
                        report,
                        VerificationIssueKind::Mismatch,
                        "cpu",
                        option,
                        &unit.name,
                        expected,
                        None,
                        "persistent PM QoS latency descriptor differs",
                    );
                }
            }
            _ => {}
        }
    }
}

fn verify_boost(
    report: &mut VerificationReport,
    option: &str,
    expected: &str,
    policies: &[PathBuf],
) {
    let expected = match parse_bool(expected) {
        Some(value) => value,
        None => {
            issue(
                report,
                VerificationIssueKind::Unsupported,
                "cpu",
                option,
                "boost",
                expected,
                None,
                "invalid boolean boost value",
            );
            return;
        }
    };
    let targets = policies
        .iter()
        .map(|path| path.join("boost"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    if !targets.is_empty() {
        check_paths(
            report,
            "cpu",
            option,
            if expected { "1" } else { "0" },
            targets,
            ValueMode::Exact,
        );
        return;
    }
    let no_turbo = rooted("/sys/devices/system/cpu/intel_pstate/no_turbo");
    check_paths(
        report,
        "cpu",
        option,
        if expected { "0" } else { "1" },
        no_turbo.is_file().then_some(no_turbo).into_iter().collect(),
        ValueMode::Exact,
    );
}

fn verify_no_turbo(report: &mut VerificationReport, option: &str, expected: &str) {
    let Some(expected) = parse_bool(expected) else {
        issue(
            report,
            VerificationIssueKind::Unsupported,
            "cpu",
            option,
            "no_turbo",
            expected,
            None,
            "invalid boolean no_turbo value",
        );
        return;
    };
    check_paths(
        report,
        "cpu",
        option,
        if expected { "1" } else { "0" },
        [rooted("/sys/devices/system/cpu/intel_pstate/no_turbo")]
            .into_iter()
            .filter(|path| path.is_file())
            .collect(),
        ValueMode::Exact,
    );
}

fn verify_sysctl(unit: &ProfileUnit, report: &mut VerificationReport) {
    for (option, expected) in &unit.options {
        if !valid_sysctl_key(option) {
            issue(
                report,
                VerificationIssueKind::Unsupported,
                "sysctl",
                option,
                &unit.name,
                expected,
                None,
                "invalid sysctl key",
            );
            continue;
        }
        check_file(
            report,
            "sysctl",
            option,
            rooted(&format!("/proc/sys/{}", option.replace('.', "/"))),
            expected,
            ValueMode::Assignment,
        );
    }
}

fn verify_vm(unit: &ProfileUnit, report: &mut VerificationReport) {
    for (option, expected) in &unit.options {
        match option.as_str() {
            "transparent_hugepages" | "transparent_hugepage" => check_file(
                report,
                "vm",
                option,
                thp_path("enabled"),
                expected,
                ValueMode::Choice,
            ),
            "transparent_hugepage.defrag" => check_file(
                report,
                "vm",
                option,
                thp_path("defrag"),
                expected,
                ValueMode::Choice,
            ),
            "dirty_ratio" | "dirty_background_ratio" | "dirty_bytes" | "dirty_background_bytes" => {
                let expected = if expected.trim().ends_with('%') {
                    match percentage_memory(expected) {
                        Some(value) => value,
                        None => {
                            issue(
                                report,
                                VerificationIssueKind::ReadError,
                                "vm",
                                option,
                                "/proc/meminfo",
                                expected,
                                None,
                                "cannot resolve percentage against MemTotal",
                            );
                            continue;
                        }
                    }
                } else {
                    expected.clone()
                };
                check_file(
                    report,
                    "vm",
                    option,
                    rooted(&format!("/proc/sys/vm/{option}")),
                    &expected,
                    ValueMode::Assignment,
                );
            }
            _ => {}
        }
    }
}

fn verify_disk(unit: &ProfileUnit, report: &mut VerificationReport) {
    let devices = if unit.devices == "*" {
        matching_children(&rooted("/sys/block"), |name| {
            !(name.starts_with("loop")
                || name.starts_with("ram")
                || name.starts_with("fd")
                || name.starts_with("dm-")
                || name.starts_with("sr"))
        })
        .into_iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect::<Vec<_>>()
    } else {
        unit.devices
            .split([',', ' '])
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect()
    };

    for (option, expected) in &unit.options {
        for device in &devices {
            let (leaf, mode, expected) = match option.as_str() {
                "elevator" => ("scheduler", ValueMode::Choice, expected.clone()),
                "readahead" => (
                    "read_ahead_kb",
                    ValueMode::Assignment,
                    normalize_readahead(expected),
                ),
                "scheduler_quantum" => ("iosched/quantum", ValueMode::Exact, expected.clone()),
                _ => continue,
            };
            check_file(
                report,
                "disk",
                option,
                rooted(&format!("/sys/block/{device}/queue/{leaf}")),
                &expected,
                mode,
            );
        }
    }
}

fn verify_acpi(unit: &ProfileUnit, report: &mut VerificationReport) {
    if let Some(expected) = option_value(&unit.options, "platform_profile") {
        check_file(
            report,
            "acpi",
            "platform_profile",
            rooted("/sys/firmware/acpi/platform_profile"),
            expected,
            ValueMode::Choice,
        );
    }
}

fn verify_network(unit: &ProfileUnit, report: &mut VerificationReport) {
    for (option, expected) in &unit.options {
        if matches!(
            option.as_str(),
            "features"
                | "coalesce"
                | "pause"
                | "ring"
                | "channels"
                | "wake_on_lan"
                | "mtu"
                | "txqueuelen"
                | "dynamic"
        ) {
            if !tuning::network::verify_device_option(&unit.devices, option, expected, true) {
                issue(
                    report,
                    VerificationIssueKind::Mismatch,
                    "network",
                    option,
                    &unit.name,
                    expected,
                    None,
                    "network device settings do not match",
                );
            }
            continue;
        }
        if option == "nf_conntrack_hashsize" {
            check_file(
                report,
                "network",
                option,
                rooted("/sys/module/nf_conntrack/parameters/hashsize"),
                expected,
                ValueMode::Assignment,
            );
            continue;
        }
        let relative = match option.as_str() {
            "tcp_congestion_control" => "ipv4/tcp_congestion_control",
            "tcp_window_scaling" => "ipv4/tcp_window_scaling",
            "tcp_timestamps" => "ipv4/tcp_timestamps",
            "tcp_sack" => "ipv4/tcp_sack",
            "tcp_fastopen" => "ipv4/tcp_fastopen",
            "tcp_rmem" => "ipv4/tcp_rmem",
            "tcp_wmem" => "ipv4/tcp_wmem",
            "tcp_max_syn_backlog" => "ipv4/tcp_max_syn_backlog",
            "tcp_tw_reuse" => "ipv4/tcp_tw_reuse",
            "tcp_fin_timeout" => "ipv4/tcp_fin_timeout",
            "core_rmem_max" => "core/rmem_max",
            "core_wmem_max" => "core/wmem_max",
            "core_netdev_max_backlog" => "core/netdev_max_backlog",
            "core_somaxconn" => "core/somaxconn",
            _ => continue,
        };
        check_file(
            report,
            "network",
            option,
            rooted(&format!("/proc/sys/net/{relative}")),
            expected,
            ValueMode::Assignment,
        );
    }
}

fn verify_modules(unit: &ProfileUnit, report: &mut VerificationReport) {
    for (module, raw) in &unit.options {
        let module_path = rooted(&format!("/sys/module/{module}"));
        if !module_path.is_dir() {
            missing(report, "modules", module, module_path, raw);
            continue;
        }
        let parameters = raw
            .trim()
            .strip_prefix("+r")
            .map(|value| value.trim_start().strip_prefix(',').unwrap_or(value).trim())
            .unwrap_or(raw.trim());
        for assignment in parameters.split_whitespace() {
            let Some((name, expected)) = assignment.split_once('=') else {
                continue;
            };
            check_file(
                report,
                "modules",
                name,
                module_path.join("parameters").join(name),
                expected,
                ValueMode::Exact,
            );
        }
    }
}

fn verify_audio(unit: &ProfileUnit, report: &mut VerificationReport) {
    let timeout = option_value(&unit.options, "timeout").unwrap_or("0");
    let reset = option_value(&unit.options, "reset_controller")
        .and_then(parse_bool)
        .unwrap_or(true);
    let mut targets = Vec::new();
    let modules = crate::device_matcher::filter_names(
        &unit.devices,
        ["snd_hda_intel", "snd_ac97_codec"].map(str::to_string),
    );
    for module in modules {
        let base = rooted(&format!("/sys/module/{module}/parameters"));
        let timeout_path = base.join("power_save");
        if timeout_path.is_file() {
            targets.push((timeout_path, timeout.to_string(), "timeout"));
        }
        let reset_path = base.join("power_save_controller");
        if reset_path.is_file() {
            targets.push((
                reset_path,
                if reset { "1" } else { "0" }.to_string(),
                "reset_controller",
            ));
        }
    }
    if targets.is_empty() {
        missing(
            report,
            "audio",
            "controls",
            rooted("/sys/module/snd_hda_intel/parameters"),
            "audio power controls",
        );
    }
    for (path, expected, option) in targets {
        check_file(report, "audio", option, path, &expected, ValueMode::Exact);
    }
}

fn verify_video(unit: &ProfileUnit, report: &mut VerificationReport) {
    let result = tuning::video::verify_options(&unit.devices, &unit.options, false);
    report.checked += 1;
    if !result {
        report.issues.push(VerificationIssue {
            kind: VerificationIssueKind::Mismatch,
            plugin: "video".to_string(),
            option: "unit".to_string(),
            target: unit.name.clone(),
            expected: format!("{:?}", unit.options),
            actual: None,
            detail: "one or more video controls differ or are missing".to_string(),
        });
    }
}

fn verify_scsi_host(unit: &ProfileUnit, report: &mut VerificationReport) {
    let result = tuning::scsi_host::verify_options(&unit.devices, &unit.options, false);
    report.checked += 1;
    if !result {
        report.issues.push(VerificationIssue {
            kind: VerificationIssueKind::Mismatch,
            plugin: "scsi_host".to_string(),
            option: "alpm".to_string(),
            target: unit.name.clone(),
            expected: option_value(&unit.options, "alpm")
                .unwrap_or_default()
                .to_string(),
            actual: None,
            detail: "SCSI host ALPM differs or is missing".to_string(),
        });
    }
}

fn verify_script(unit: &ProfileUnit, report: &mut VerificationReport) {
    let Some(raw) = option_value(&unit.options, "script") else {
        return;
    };
    for path in raw.lines().flat_map(|line| line.split(';')).map(str::trim) {
        if path.is_empty() {
            continue;
        }
        let path = PathBuf::from(path);
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 => {
                report.checked += 1;
            }
            Ok(_) => issue(
                report,
                VerificationIssueKind::Mismatch,
                "script",
                "script",
                &path.display().to_string(),
                "executable regular file",
                None,
                "profile script is not executable",
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing(report, "script", "script", path, "executable script")
            }
            Err(error) => issue(
                report,
                VerificationIssueKind::ReadError,
                "script",
                "script",
                &path.display().to_string(),
                "executable script",
                None,
                error.to_string(),
            ),
        }
    }
    match tuning::script::verify_scripts(raw, false) {
        Ok(true) => {}
        Ok(false) => issue(
            report,
            VerificationIssueKind::Mismatch,
            "script",
            "verify",
            &unit.name,
            "successful verify action",
            None,
            "profile script verify action failed",
        ),
        Err(error) => issue(
            report,
            VerificationIssueKind::ReadError,
            "script",
            "verify",
            &unit.name,
            "successful verify action",
            None,
            error.to_string(),
        ),
    }
}

fn verify_service(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::service::verify_options(&unit.options, true) {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "service",
            "state",
            &unit.name,
            "configured service states and overlays",
            None,
            "one or more service states or configuration overlays differ",
        );
    }
}

fn verify_mounts(unit: &ProfileUnit, report: &mut VerificationReport) {
    if !tuning::mounts::verify_options(&unit.devices, &unit.options, true) {
        issue(
            report,
            VerificationIssueKind::Mismatch,
            "mounts",
            "disable_barriers",
            &unit.name,
            "disabled barriers on selected ext filesystems",
            None,
            "one or more selected mount points still has barriers enabled",
        );
    }
}

fn check_paths(
    report: &mut VerificationReport,
    plugin: &str,
    option: &str,
    expected: &str,
    paths: Vec<PathBuf>,
    mode: ValueMode,
) {
    if paths.is_empty() {
        missing(report, plugin, option, rooted("/sys"), expected);
        return;
    }
    for path in paths {
        check_file(report, plugin, option, path, expected, mode);
    }
}

fn check_paths_with(
    report: &mut VerificationReport,
    plugin: &str,
    option: &str,
    expected: &str,
    paths: Vec<PathBuf>,
    matcher: fn(&str, &str) -> bool,
) {
    if paths.is_empty() {
        missing(report, plugin, option, rooted("/sys"), expected);
        return;
    }
    for path in paths {
        report.checked += 1;
        match fs::read_to_string(&path) {
            Ok(actual) if matcher(expected, actual.trim()) => {}
            Ok(actual) => report.issues.push(VerificationIssue {
                kind: VerificationIssueKind::Mismatch,
                plugin: plugin.to_string(),
                option: option.to_string(),
                target: path.display().to_string(),
                expected: expected.to_string(),
                actual: Some(actual.trim().to_string()),
                detail: "live value differs from the resolved unit".to_string(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                report.issues.push(VerificationIssue {
                    kind: VerificationIssueKind::Missing,
                    plugin: plugin.to_string(),
                    option: option.to_string(),
                    target: path.display().to_string(),
                    expected: expected.to_string(),
                    actual: None,
                    detail: "target is not available on this system".to_string(),
                });
            }
            Err(error) => report.issues.push(VerificationIssue {
                kind: VerificationIssueKind::ReadError,
                plugin: plugin.to_string(),
                option: option.to_string(),
                target: path.display().to_string(),
                expected: expected.to_string(),
                actual: None,
                detail: error.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ValueMode {
    Exact,
    Choice,
    Assignment,
}

fn check_file(
    report: &mut VerificationReport,
    plugin: &str,
    option: &str,
    path: PathBuf,
    expected: &str,
    mode: ValueMode,
) {
    report.checked += 1;
    match fs::read_to_string(&path) {
        Ok(actual) => {
            let actual = actual.trim();
            if !value_matches(expected, actual, mode) {
                report.issues.push(VerificationIssue {
                    kind: VerificationIssueKind::Mismatch,
                    plugin: plugin.to_string(),
                    option: option.to_string(),
                    target: path.display().to_string(),
                    expected: expected.to_string(),
                    actual: Some(actual.to_string()),
                    detail: "live value differs from the resolved unit".to_string(),
                });
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            report.issues.push(VerificationIssue {
                kind: VerificationIssueKind::Missing,
                plugin: plugin.to_string(),
                option: option.to_string(),
                target: path.display().to_string(),
                expected: expected.to_string(),
                actual: None,
                detail: "target is not available on this system".to_string(),
            });
        }
        Err(error) => report.issues.push(VerificationIssue {
            kind: VerificationIssueKind::ReadError,
            plugin: plugin.to_string(),
            option: option.to_string(),
            target: path.display().to_string(),
            expected: expected.to_string(),
            actual: None,
            detail: error.to_string(),
        }),
    }
}

fn value_matches(expected: &str, actual: &str, mode: ValueMode) -> bool {
    let actual = active_value(actual);
    match mode {
        ValueMode::Exact => normalize(expected) == normalize(actual),
        ValueMode::Choice => expected
            .split('|')
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
            .any(|candidate| normalize(candidate) == normalize(actual)),
        ValueMode::Assignment => assignment_matches(expected, actual),
    }
}

fn assignment_matches(expected: &str, actual: &str) -> bool {
    for (prefix, comparison) in [
        (">=", 1),
        ("=>", 1),
        ("<=", -1),
        ("=<", -1),
        (">", 1),
        ("<", -1),
    ] {
        if let Some(target) = expected.trim().strip_prefix(prefix) {
            return match (target.trim().parse::<i64>(), actual.trim().parse::<i64>()) {
                (Ok(target), Ok(actual)) if comparison > 0 => actual >= target,
                (Ok(target), Ok(actual)) => actual <= target,
                _ => normalize(target) == normalize(actual),
            };
        }
    }
    normalize(expected) == normalize(actual)
}

fn energy_bias_matches(expected: &str, actual: &str) -> bool {
    let actual = canonical_energy_bias(actual);
    expected
        .split('|')
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
        .any(|candidate| canonical_energy_bias(candidate) == actual)
}

fn canonical_energy_bias(value: &str) -> String {
    match value.trim() {
        "0" | "performance" => "performance",
        "4" | "balance-performance" | "balance_performance" => "balance-performance",
        "6" | "normal" => "normal",
        "8" | "balance-power" | "balance_power" => "balance-power",
        "15" | "power" | "powersave" => "power",
        other => other,
    }
    .to_string()
}

fn active_value(raw: &str) -> &str {
    let Some(start) = raw.find('[') else {
        return raw.trim();
    };
    let Some(end) = raw[start + 1..].find(']') else {
        return raw.trim();
    };
    raw[start + 1..start + 1 + end].trim()
}

fn normalize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_readahead(raw: &str) -> String {
    let trimmed = raw.trim();
    let (prefix, value) = [">=", "=>", "<=", "=<", ">", "<"]
        .into_iter()
        .find_map(|prefix| {
            trimmed
                .strip_prefix(prefix)
                .map(|value| (prefix, value.trim()))
        })
        .unwrap_or(("", trimmed));
    let mut parts = value.split_whitespace();
    let number = parts.next().unwrap_or_default().parse::<i64>().unwrap_or(0);
    let number = if parts.next() == Some("s") {
        number / 2
    } else {
        number
    };
    format!("{prefix}{number}")
}

fn percentage_memory(raw: &str) -> Option<String> {
    let percent = raw.trim().trim_end_matches('%').parse::<u64>().ok()?;
    let meminfo = fs::read_to_string(rooted("/proc/meminfo")).ok()?;
    let kilobytes = meminfo.lines().find_map(|line| {
        line.strip_prefix("MemTotal:")
            .and_then(|value| value.trim().trim_end_matches(" kB").parse::<u64>().ok())
    })?;
    Some(
        kilobytes
            .saturating_mul(1024)
            .saturating_mul(percent)
            .checked_div(100)
            .unwrap_or(0)
            .to_string(),
    )
}

fn thp_path(leaf: &str) -> PathBuf {
    for base in [
        "/sys/kernel/mm/transparent_hugepage",
        "/sys/kernel/mm/redhat_transparent_hugepage",
    ] {
        let path = rooted(base);
        if path.is_dir() {
            return path.join(leaf);
        }
    }
    rooted(&format!("/sys/kernel/mm/transparent_hugepage/{leaf}"))
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

fn valid_sysctl_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 256
        && !key.starts_with('.')
        && !key.ends_with('.')
        && !key.contains("..")
        && key.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "y" | "yes" | "t" | "true" | "on" => Some(true),
        "0" | "n" | "no" | "f" | "false" | "off" => Some(false),
        _ => None,
    }
}

fn is_conditional(unit: &ProfileUnit) -> bool {
    unit.cpuinfo_regex.is_some() || unit.uname_regex.is_some()
}

fn rooted(path: &str) -> PathBuf {
    config::resolve_path(path)
}

fn missing(
    report: &mut VerificationReport,
    plugin: &str,
    option: &str,
    path: PathBuf,
    expected: &str,
) {
    issue(
        report,
        VerificationIssueKind::Missing,
        plugin,
        option,
        &path.display().to_string(),
        expected,
        None,
        "target is not available on this system",
    );
}

#[allow(clippy::too_many_arguments)]
fn issue(
    report: &mut VerificationReport,
    kind: VerificationIssueKind,
    plugin: &str,
    option: &str,
    target: &str,
    expected: &str,
    actual: Option<String>,
    detail: impl Into<String>,
) {
    report.checked += 1;
    report.issues.push(VerificationIssue {
        kind,
        plugin: plugin.to_string(),
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
    fn assignment_comparisons_are_fail_closed() {
        assert!(assignment_matches("=>4096", "8192"));
        assert!(!assignment_matches("=>4096", "2048"));
        assert!(assignment_matches("=<100", "80"));
        assert!(!assignment_matches("=<100", "120"));
    }

    #[test]
    fn energy_bias_names_and_numeric_values_are_equivalent() {
        assert!(energy_bias_matches("normal|powersave", "6"));
        assert!(energy_bias_matches("powersave", "15"));
        assert!(!energy_bias_matches("performance", "15"));
    }

    #[test]
    fn normalizes_sector_readahead_units() {
        assert_eq!(normalize_readahead("=>8192 s"), "=>4096");
        assert_eq!(normalize_readahead("4096"), "4096");
    }
}
