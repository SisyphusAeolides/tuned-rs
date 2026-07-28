use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginOption {
    pub name: &'static str,
    pub default_value: &'static str,
    pub hint: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginDescriptor {
    pub name: &'static str,
    pub documentation: &'static str,
    pub options: &'static [PluginOption],
}

const CPU_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "governor",
        default_value: "",
        hint: "CPU frequency governor applied to matching processors.",
    },
    PluginOption {
        name: "energy_perf_bias",
        default_value: "",
        hint: "CPU energy-performance bias with ordered fallback values.",
    },
    PluginOption {
        name: "energy_performance_preference",
        default_value: "",
        hint: "Energy-performance preference applied through cpufreq EPP.",
    },
    PluginOption {
        name: "min_perf_pct",
        default_value: "",
        hint: "Minimum P-state performance percentage.",
    },
    PluginOption {
        name: "max_perf_pct",
        default_value: "",
        hint: "Maximum P-state performance percentage.",
    },
    PluginOption {
        name: "boost",
        default_value: "",
        hint: "CPU frequency boost or Intel turbo switch.",
    },
    PluginOption {
        name: "force_latency",
        default_value: "",
        hint: "Persistent CPU idle-latency constraint.",
    },
    PluginOption {
        name: "pm_qos_resume_latency_us",
        default_value: "",
        hint: "Per-CPU PM QoS resume latency in microseconds.",
    },
    PluginOption {
        name: "sampling_down_factor",
        default_value: "",
        hint: "Governor sampling-down multiplier.",
    },
];

const VM_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "transparent_hugepages",
        default_value: "",
        hint: "Transparent huge-page enable policy.",
    },
    PluginOption {
        name: "transparent_hugepage.defrag",
        default_value: "",
        hint: "Transparent huge-page defragmentation policy.",
    },
    PluginOption {
        name: "dirty_bytes",
        default_value: "",
        hint: "Absolute or percentage dirty-memory limit.",
    },
    PluginOption {
        name: "dirty_ratio",
        default_value: "",
        hint: "Dirty-memory ratio.",
    },
    PluginOption {
        name: "dirty_background_bytes",
        default_value: "",
        hint: "Absolute or percentage background writeback threshold.",
    },
    PluginOption {
        name: "dirty_background_ratio",
        default_value: "",
        hint: "Background writeback ratio.",
    },
];

const DISK_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "devices",
        default_value: "*",
        hint: "Comma-separated block-device match expression.",
    },
    PluginOption {
        name: "elevator",
        default_value: "",
        hint: "I/O scheduler selected for matching block devices.",
    },
    PluginOption {
        name: "readahead",
        default_value: "",
        hint: "Read-ahead size, including TuneD comparison modifiers.",
    },
];

const ACPI_OPTIONS: &[PluginOption] = &[PluginOption {
    name: "platform_profile",
    default_value: "",
    hint: "ACPI platform profile with optional vertical-bar fallbacks.",
}];

const NETWORK_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "tcp_congestion_control",
        default_value: "",
        hint: "TCP congestion-control algorithm.",
    },
    PluginOption {
        name: "tcp_window_scaling",
        default_value: "",
        hint: "TCP window-scaling switch.",
    },
    PluginOption {
        name: "tcp_timestamps",
        default_value: "",
        hint: "TCP timestamp switch.",
    },
    PluginOption {
        name: "tcp_sack",
        default_value: "",
        hint: "TCP selective-acknowledgement switch.",
    },
    PluginOption {
        name: "tcp_fastopen",
        default_value: "",
        hint: "TCP Fast Open mode.",
    },
    PluginOption {
        name: "tcp_rmem",
        default_value: "",
        hint: "TCP receive-buffer triplet.",
    },
    PluginOption {
        name: "tcp_wmem",
        default_value: "",
        hint: "TCP transmit-buffer triplet.",
    },
    PluginOption {
        name: "tcp_max_syn_backlog",
        default_value: "",
        hint: "Maximum queued connection requests.",
    },
    PluginOption {
        name: "tcp_tw_reuse",
        default_value: "",
        hint: "TCP TIME-WAIT socket reuse policy.",
    },
    PluginOption {
        name: "tcp_fin_timeout",
        default_value: "",
        hint: "TCP FIN timeout.",
    },
    PluginOption {
        name: "core_rmem_max",
        default_value: "",
        hint: "Maximum core socket receive buffer.",
    },
    PluginOption {
        name: "core_wmem_max",
        default_value: "",
        hint: "Maximum core socket transmit buffer.",
    },
    PluginOption {
        name: "core_netdev_max_backlog",
        default_value: "",
        hint: "Maximum network receive backlog.",
    },
    PluginOption {
        name: "core_somaxconn",
        default_value: "",
        hint: "Maximum listen socket backlog.",
    },
];

const AUDIO_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "timeout",
        default_value: "0",
        hint: "Audio codec power-save timeout in seconds.",
    },
    PluginOption {
        name: "reset_controller",
        default_value: "true",
        hint: "Reset the supported audio controller when power saving changes.",
    },
];

const VIDEO_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "radeon_powersave",
        default_value: "",
        hint: "Ordered Radeon power-method and DPM fallback list.",
    },
    PluginOption {
        name: "panel_power_savings",
        default_value: "",
        hint: "amdgpu panel power-savings level from zero through four.",
    },
];

const SCSI_HOST_OPTIONS: &[PluginOption] = &[PluginOption {
    name: "alpm",
    default_value: "",
    hint: "SATA Aggressive Link Power Management policy.",
}];

const SELINUX_OPTIONS: &[PluginOption] = &[PluginOption {
    name: "avc_cache_threshold",
    default_value: "",
    hint: "Maximum number of entries retained in the SELinux access-vector cache.",
}];

const USB_OPTIONS: &[PluginOption] = &[PluginOption {
    name: "autosuspend",
    default_value: "",
    hint: "USB autosuspend switch for matching USB devices.",
}];

const SYSTEMD_OPTIONS: &[PluginOption] = &[PluginOption {
    name: "cpu_affinity",
    default_value: "",
    hint: "Default CPU affinity inherited by the systemd manager and its services.",
}];

const UNCORE_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "max_freq_khz",
        default_value: "",
        hint: "Maximum Intel uncore frequency in kHz or as a hardware-range percentage.",
    },
    PluginOption {
        name: "min_freq_khz",
        default_value: "",
        hint: "Minimum Intel uncore frequency in kHz or as a hardware-range percentage.",
    },
];

const SCRIPT_OPTIONS: &[PluginOption] = &[PluginOption {
    name: "script",
    default_value: "",
    hint: "Executable inside a configured profile directory.",
}];

const GPU_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "amd_power_profile",
        default_value: "",
        hint: "AMD GPU power-performance profile.",
    },
    PluginOption {
        name: "amd_power_dpm_force_performance_level",
        default_value: "",
        hint: "AMD GPU forced DPM performance level.",
    },
    PluginOption {
        name: "nvidia_power_limit",
        default_value: "",
        hint: "NVIDIA GPU power limit in watts.",
    },
    PluginOption {
        name: "nvidia_graphics_clock",
        default_value: "",
        hint: "NVIDIA graphics-clock lock in MHz.",
    },
    PluginOption {
        name: "nvidia_memory_clock",
        default_value: "",
        hint: "NVIDIA memory-clock lock in MHz.",
    },
    PluginOption {
        name: "nvidia_persistence_mode",
        default_value: "",
        hint: "NVIDIA persistence-mode switch.",
    },
];

const STORAGE_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "nvme_apst",
        default_value: "",
        hint: "NVMe autonomous power-state transition latency policy.",
    },
    PluginOption {
        name: "ssd_trim",
        default_value: "",
        hint: "SSD discard scheduling policy.",
    },
    PluginOption {
        name: "io_scheduler",
        default_value: "",
        hint: "I/O scheduler for discovered storage devices.",
    },
    PluginOption {
        name: "nr_requests",
        default_value: "",
        hint: "Block queue request depth.",
    },
];

const THERMAL_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "cpu_temp_limit",
        default_value: "",
        hint: "CPU thermal limit in degrees Celsius.",
    },
    PluginOption {
        name: "fan_control",
        default_value: "auto",
        hint: "Fan-controller operating mode.",
    },
    PluginOption {
        name: "thermal_policy",
        default_value: "",
        hint: "Kernel thermal-zone policy.",
    },
    PluginOption {
        name: "trip_point",
        default_value: "",
        hint: "Thermal trip point in degrees Celsius.",
    },
];

const BATTERY_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "charge_start_threshold",
        default_value: "",
        hint: "Battery charge-start threshold in percent.",
    },
    PluginOption {
        name: "charge_stop_threshold",
        default_value: "",
        hint: "Battery charge-stop threshold in percent.",
    },
    PluginOption {
        name: "conservation_mode",
        default_value: "",
        hint: "Vendor conservation-mode switch.",
    },
    PluginOption {
        name: "battery_care_limit",
        default_value: "",
        hint: "Battery care charge limit in percent.",
    },
];

const HERMES_OPTIONS: &[PluginOption] = &[
    PluginOption {
        name: "cmd_ring_size",
        default_value: "",
        hint: "Hermes command-ring size.",
    },
    PluginOption {
        name: "rsp_ring_size",
        default_value: "",
        hint: "Hermes response-ring size.",
    },
    PluginOption {
        name: "ring_overflow_threshold",
        default_value: "",
        hint: "Hermes ring-overflow recovery threshold.",
    },
    PluginOption {
        name: "ring_poll_interval",
        default_value: "",
        hint: "Hermes ring polling interval.",
    },
    PluginOption {
        name: "runtime_pm_enabled",
        default_value: "",
        hint: "Hermes runtime power-management switch.",
    },
    PluginOption {
        name: "idle_timeout_ms",
        default_value: "",
        hint: "Hermes idle timeout in milliseconds.",
    },
    PluginOption {
        name: "autosuspend_delay",
        default_value: "",
        hint: "Hermes autosuspend delay.",
    },
    PluginOption {
        name: "debug_level",
        default_value: "",
        hint: "Hermes diagnostic verbosity.",
    },
    PluginOption {
        name: "error_recovery_mode",
        default_value: "",
        hint: "Hermes error-recovery policy.",
    },
    PluginOption {
        name: "firmware_validation",
        default_value: "",
        hint: "Hermes firmware-validation policy or allow-list path.",
    },
    PluginOption {
        name: "display_heads",
        default_value: "",
        hint: "Maximum number of enabled Hermes display heads.",
    },
    PluginOption {
        name: "gsp_power_mode",
        default_value: "balanced",
        hint: "Hermes GSP power policy.",
    },
];

pub const PLUGINS: &[PluginDescriptor] = &[
    PluginDescriptor {
        name: "modules",
        documentation: "Writes kernel module parameters and optionally reloads modules.",
        options: &[],
    },
    PluginDescriptor {
        name: "cpu",
        documentation: "Controls CPU frequency, P-state, boost, and latency policy.",
        options: CPU_OPTIONS,
    },
    PluginDescriptor {
        name: "sysctl",
        documentation: "Applies arbitrary kernel sysctl assignments with TuneD modifiers.",
        options: &[],
    },
    PluginDescriptor {
        name: "vm",
        documentation: "Controls virtual-memory writeback and transparent huge-page policy.",
        options: VM_OPTIONS,
    },
    PluginDescriptor {
        name: "disk",
        documentation: "Controls block-device scheduler and read-ahead settings.",
        options: DISK_OPTIONS,
    },
    PluginDescriptor {
        name: "acpi",
        documentation: "Selects the ACPI platform performance profile.",
        options: ACPI_OPTIONS,
    },
    PluginDescriptor {
        name: "net",
        documentation: "Provides the upstream TuneD network plugin identity.",
        options: NETWORK_OPTIONS,
    },
    PluginDescriptor {
        name: "network",
        documentation: "Controls global TCP stack policy.",
        options: NETWORK_OPTIONS,
    },
    PluginDescriptor {
        name: "audio",
        documentation: "Controls supported audio codec power-saving parameters.",
        options: AUDIO_OPTIONS,
    },
    PluginDescriptor {
        name: "video",
        documentation: "Controls Radeon power policy and amdgpu panel power savings.",
        options: VIDEO_OPTIONS,
    },
    PluginDescriptor {
        name: "scsi_host",
        documentation: "Controls SATA link power management on supported SCSI hosts.",
        options: SCSI_HOST_OPTIONS,
    },
    PluginDescriptor {
        name: "selinux",
        documentation: "Controls the SELinux access-vector cache threshold.",
        options: SELINUX_OPTIONS,
    },
    PluginDescriptor {
        name: "usb",
        documentation: "Controls autosuspend for matching USB devices.",
        options: USB_OPTIONS,
    },
    PluginDescriptor {
        name: "systemd",
        documentation: "Controls systemd manager defaults through TuneD's system.conf drop-in.",
        options: SYSTEMD_OPTIONS,
    },
    PluginDescriptor {
        name: "uncore",
        documentation: "Controls Intel uncore frequency limits for matching uncore devices.",
        options: UNCORE_OPTIONS,
    },
    PluginDescriptor {
        name: "script",
        documentation: "Runs profile-local compatibility scripts for start, verify, and stop.",
        options: SCRIPT_OPTIONS,
    },
    PluginDescriptor {
        name: "gpu",
        documentation: "Controls AMD and NVIDIA GPU power and clock policy.",
        options: GPU_OPTIONS,
    },
    PluginDescriptor {
        name: "storage",
        documentation: "Controls NVMe power policy and block queue behavior.",
        options: STORAGE_OPTIONS,
    },
    PluginDescriptor {
        name: "thermal",
        documentation: "Controls thermal-zone, trip-point, and fan policy.",
        options: THERMAL_OPTIONS,
    },
    PluginDescriptor {
        name: "battery",
        documentation: "Controls supported battery thresholds and conservation modes.",
        options: BATTERY_OPTIONS,
    },
    PluginDescriptor {
        name: "hermes",
        documentation: "Controls Hermes GSP runtime, ring, display, and recovery policy.",
        options: HERMES_OPTIONS,
    },
];

pub fn descriptor(name: &str) -> Option<&'static PluginDescriptor> {
    PLUGINS.iter().find(|plugin| plugin.name == name)
}

pub fn all_options() -> HashMap<String, HashMap<String, String>> {
    PLUGINS
        .iter()
        .map(|plugin| {
            let options = plugin
                .options
                .iter()
                .map(|option| (option.name.to_string(), option.default_value.to_string()))
                .collect();
            (plugin.name.to_string(), options)
        })
        .collect()
}

pub fn documentation(name: &str) -> String {
    descriptor(name)
        .map(|plugin| plugin.documentation.to_string())
        .unwrap_or_default()
}

pub fn hints(name: &str) -> HashMap<String, String> {
    descriptor(name)
        .map(|plugin| {
            plugin
                .options
                .iter()
                .map(|option| (option.name.to_string(), option.hint.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn registry_names_and_options_are_unique() {
        let mut plugin_names = HashSet::new();
        for plugin in PLUGINS {
            assert!(plugin_names.insert(plugin.name));
            assert!(!plugin.documentation.is_empty());

            let mut option_names = HashSet::new();
            for option in plugin.options {
                assert!(option_names.insert(option.name));
                assert!(!option.hint.is_empty());
            }
        }
    }

    #[test]
    fn registry_exposes_every_runtime_plugin() {
        let names = PLUGINS
            .iter()
            .map(|plugin| plugin.name)
            .collect::<HashSet<_>>();
        for expected in [
            "modules",
            "cpu",
            "sysctl",
            "vm",
            "disk",
            "acpi",
            "net",
            "network",
            "audio",
            "video",
            "scsi_host",
            "selinux",
            "usb",
            "systemd",
            "uncore",
            "script",
            "gpu",
            "storage",
            "thermal",
            "battery",
            "hermes",
        ] {
            assert!(names.contains(expected));
        }
    }
}
