pub mod acpi;
pub mod audio;
pub mod battery;
pub mod cpu;
pub mod disk;
pub mod gpu;
pub mod hermes;
pub mod modifiers;
pub mod modules;
pub mod network;
pub mod scsi_host;
pub mod script;
pub mod storage;
pub mod sysctl;
pub mod sysfs;
pub mod thermal;
pub mod video;
pub mod vm;

use anyhow::Result;
use tracing::warn;

use crate::profile::{DiskSettings, NetworkSettings, Profile, VmSettings};
use crate::profile_units::ProfileUnit;
use crate::rollback::Rollback;

pub fn apply_profile(rollback: &Rollback, profile: &Profile) -> Result<()> {
    apply_module_units(rollback, profile)?;

    if let Some(governor) = &profile.cpu.governor {
        cpu::apply_governor(rollback, governor)?;
    }
    if let Some(energy_perf_bias) = &profile.cpu.energy_perf_bias {
        cpu::apply_energy_perf_bias(rollback, energy_perf_bias)?;
    }
    if let Some(epp) = &profile.cpu.energy_performance_preference {
        cpu::apply_epp(rollback, epp)?;
    }
    if let Some(minimum) = &profile.cpu.min_perf_pct {
        cpu::apply_min_perf_pct(rollback, minimum)?;
    }
    if let Some(maximum) = &profile.cpu.max_perf_pct {
        cpu::apply_max_perf_pct(rollback, maximum)?;
    }
    if let Some(boost) = &profile.cpu.boost {
        cpu::apply_boost(rollback, boost)?;
    }
    if let Some(latency) = &profile.cpu.pm_qos_resume_latency_us {
        cpu::apply_pm_qos_resume_latency_us(rollback, latency)?;
    }
    if let Some(factor) = &profile.cpu.sampling_down_factor {
        cpu::apply_sampling_down_factor(rollback, factor)?;
    }
    if let Some(force_latency) = &profile.cpu.force_latency {
        cpu::apply_force_latency(rollback, force_latency)?;
    }

    for (key, value) in &profile.sysctl {
        sysctl::apply_option(rollback, key, value)?;
    }

    vm::apply_options(rollback, &vm_option_pairs(&profile.vm))?;
    apply_scsi_host_units(rollback, profile)?;
    disk::apply_options(
        rollback,
        profile.disk.devices.as_deref(),
        &disk_option_pairs(&profile.disk),
    )?;

    if let Some(platform_profile) = &profile.acpi.platform_profile {
        acpi::apply_platform_profile(rollback, platform_profile)?;
    }

    network::apply_tcp_options(rollback, &network_option_pairs(&profile.network))?;
    apply_audio_units(rollback, profile)?;
    apply_video_units(rollback, profile)?;
    gpu::apply_gpu_options(rollback, &profile.gpu)?;
    storage::apply_storage_options(rollback, &profile.storage)?;
    thermal::apply_thermal_options(rollback, &profile.thermal)?;
    battery::apply_battery_options(rollback, &profile.battery)?;
    hermes::apply_hermes_options(rollback, &profile.hermes)?;

    apply_script_units(rollback, profile)?;
    report_preserved_but_unimplemented_units(profile);
    Ok(())
}

fn apply_module_units(rollback: &Rollback, profile: &Profile) -> Result<()> {
    for unit in profile.units_of_type("modules").filter(|unit| unit.enabled) {
        if skip_conditional_unit(unit) {
            continue;
        }
        modules::apply_options(rollback, &unit.options)?;
    }
    Ok(())
}

fn apply_audio_units(rollback: &Rollback, profile: &Profile) -> Result<()> {
    for unit in profile.units_of_type("audio").filter(|unit| unit.enabled) {
        if skip_conditional_unit(unit) {
            continue;
        }
        audio::apply_options(rollback, &unit.options)?;
    }
    Ok(())
}

fn apply_video_units(rollback: &Rollback, profile: &Profile) -> Result<()> {
    for unit in profile.units_of_type("video").filter(|unit| unit.enabled) {
        if skip_conditional_unit(unit) {
            continue;
        }
        video::apply_options(rollback, &unit.options)?;
    }
    Ok(())
}

fn apply_scsi_host_units(rollback: &Rollback, profile: &Profile) -> Result<()> {
    for unit in profile
        .units_of_type("scsi_host")
        .filter(|unit| unit.enabled)
    {
        if skip_conditional_unit(unit) {
            continue;
        }
        scsi_host::apply_options(rollback, &unit.devices, &unit.options)?;
    }
    Ok(())
}

fn apply_script_units(rollback: &Rollback, profile: &Profile) -> Result<()> {
    for unit in profile.units_of_type("script").filter(|unit| unit.enabled) {
        if skip_conditional_unit(unit) {
            continue;
        }
        if let Some(scripts) = unit.option("script") {
            script::apply_scripts(rollback, scripts)?;
        }
    }
    Ok(())
}

fn skip_conditional_unit(unit: &ProfileUnit) -> bool {
    if unit_is_conditional(unit) {
        warn!(
            "Conditional unit '{}' of type '{}' is preserved but awaits condition evaluation",
            unit.name, unit.plugin_type
        );
        true
    } else {
        false
    }
}

fn report_preserved_but_unimplemented_units(profile: &Profile) {
    const IMPLEMENTED: &[&str] = &[
        "variables",
        "modules",
        "cpu",
        "sysctl",
        "vm",
        "disk",
        "acpi",
        "network",
        "net",
        "audio",
        "video",
        "scsi_host",
        "gpu",
        "storage",
        "thermal",
        "battery",
        "hermes",
        "script",
    ];
    for unit in profile.units.iter().filter(|unit| unit.enabled) {
        if !IMPLEMENTED.contains(&unit.plugin_type.as_str()) {
            warn!(
                "Profile unit '{}' uses preserved but unimplemented plugin type '{}'",
                unit.name, unit.plugin_type
            );
        }
    }
}

fn unit_is_conditional(unit: &ProfileUnit) -> bool {
    unit.devices_udev_regex.is_some()
        || unit.cpuinfo_regex.is_some()
        || unit.uname_regex.is_some()
}

fn vm_option_pairs(vm: &VmSettings) -> Vec<(String, String)> {
    let mut options = Vec::new();
    push_option(
        &mut options,
        "transparent_hugepages",
        &vm.transparent_hugepages,
    );
    push_option(
        &mut options,
        "transparent_hugepage.defrag",
        &vm.transparent_hugepage_defrag,
    );
    push_option(&mut options, "dirty_bytes", &vm.dirty_bytes);
    push_option(&mut options, "dirty_ratio", &vm.dirty_ratio);
    push_option(
        &mut options,
        "dirty_background_bytes",
        &vm.dirty_background_bytes,
    );
    push_option(
        &mut options,
        "dirty_background_ratio",
        &vm.dirty_background_ratio,
    );
    options
}

fn disk_option_pairs(disk: &DiskSettings) -> Vec<(String, String)> {
    let mut options = Vec::new();
    push_option(&mut options, "elevator", &disk.elevator);
    push_option(&mut options, "readahead", &disk.readahead);
    options
}

fn push_option(options: &mut Vec<(String, String)>, key: &str, value: &Option<String>) {
    if let Some(value) = value {
        options.push((key.to_string(), value.clone()));
    }
}

fn network_option_pairs(network: &NetworkSettings) -> Vec<(String, String)> {
    let mut options = Vec::new();
    push_option(
        &mut options,
        "tcp_congestion_control",
        &network.tcp_congestion_control,
    );
    push_option(
        &mut options,
        "tcp_window_scaling",
        &network.tcp_window_scaling,
    );
    push_option(&mut options, "tcp_timestamps", &network.tcp_timestamps);
    push_option(&mut options, "tcp_sack", &network.tcp_sack);
    push_option(&mut options, "tcp_fastopen", &network.tcp_fastopen);
    push_option(&mut options, "tcp_rmem", &network.tcp_rmem);
    push_option(&mut options, "tcp_wmem", &network.tcp_wmem);
    push_option(
        &mut options,
        "tcp_max_syn_backlog",
        &network.tcp_max_syn_backlog,
    );
    push_option(&mut options, "tcp_tw_reuse", &network.tcp_tw_reuse);
    push_option(&mut options, "tcp_fin_timeout", &network.tcp_fin_timeout);
    push_option(&mut options, "core_rmem_max", &network.core_rmem_max);
    push_option(&mut options, "core_wmem_max", &network.core_wmem_max);
    push_option(
        &mut options,
        "core_netdev_max_backlog",
        &network.core_netdev_max_backlog,
    );
    push_option(&mut options, "core_somaxconn", &network.core_somaxconn);
    options
}
