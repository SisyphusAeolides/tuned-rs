pub mod acpi;
pub mod audio;
pub mod battery;
pub mod bootloader;
pub mod cpu;
pub mod disk;
pub mod eeepc_she;
pub mod generic_sysfs;
pub mod gpu;
pub mod hermes;
pub mod irq;
pub mod irqbalance;
pub mod modifiers;
pub mod modules;
pub mod mounts;
pub mod network;
pub mod rtentsk;
pub mod scheduler;
pub mod script;
pub mod scsi_host;
pub mod selinux;
pub mod service;
pub mod storage;
pub mod sysctl;
pub mod sysfs;
pub mod systemd;
pub mod thermal;
pub mod uncore;
pub mod usb;
pub mod video;
pub mod vm;

use anyhow::{bail, Result};

use crate::plugins;
use crate::profile::{DiskSettings, NetworkSettings, Profile, VmSettings};
use crate::profile_runtime;
use crate::profile_units::{option_value, ProfileUnit};
use crate::rollback::Rollback;

pub fn apply_profile(rollback: &Rollback, profile: &Profile) -> Result<()> {
    let units = profile_runtime::active_units(profile)?;
    if units.is_empty() && profile.units.is_empty() {
        return apply_legacy_projection(rollback, profile);
    }

    for unit in units {
        validate_unit_contract(&unit)?;
        apply_unit(rollback, &unit)?;
    }
    Ok(())
}

fn apply_unit(rollback: &Rollback, unit: &ProfileUnit) -> Result<()> {
    match unit.plugin_type.as_str() {
        "modules" => modules::apply_options(rollback, &unit.options),
        "cpu" => apply_cpu_unit(rollback, unit),
        "sysctl" => {
            for (key, value) in &unit.options {
                sysctl::apply_option(rollback, key, value)?;
            }
            Ok(())
        }
        "sysfs" => generic_sysfs::apply_options(rollback, &unit.options),
        "vm" => vm::apply_options(rollback, &unit.options),
        "disk" => disk::apply_options(
            rollback,
            (unit.devices != "*").then_some(unit.devices.as_str()),
            &unit.options,
        ),
        "acpi" => apply_acpi_unit(rollback, unit),
        "net" | "network" => network::apply_options(rollback, &unit.devices, &unit.options),
        "audio" => audio::apply_options(rollback, &unit.options),
        "video" => video::apply_options(rollback, &unit.options),
        "scsi_host" => scsi_host::apply_options(rollback, &unit.devices, &unit.options),
        "selinux" => selinux::apply_options(rollback, &unit.options),
        "usb" => usb::apply_options(rollback, &unit.devices, &unit.options),
        "systemd" => systemd::apply_options(rollback, &unit.options),
        "uncore" => uncore::apply_options(rollback, &unit.devices, &unit.options),
        "irqbalance" => irqbalance::apply_options(rollback, &unit.options),
        "irq" => irq::apply_options(rollback, &unit.devices, &unit.options),
        "rtentsk" => rtentsk::apply(),
        "scheduler" => scheduler::apply_options(rollback, &unit.options),
        "eeepc_she" => eeepc_she::apply(&unit.options),
        "bootloader" => bootloader::apply_options(rollback, &unit.options),
        "script" => {
            if let Some(scripts) = option_value(&unit.options, "script") {
                script::apply_scripts(rollback, scripts)?;
            }
            Ok(())
        }
        "service" => service::apply_options(rollback, &unit.options),
        "mounts" => mounts::apply_options(rollback, &unit.devices, &unit.options),
        "gpu" => gpu::apply_gpu_options(rollback, &unit.options),
        "storage" => storage::apply_storage_options(rollback, &unit.options),
        "thermal" => thermal::apply_thermal_options(rollback, &unit.options),
        "battery" => battery::apply_battery_options(rollback, &unit.options),
        "hermes" => hermes::apply_hermes_options(rollback, &unit.options),
        other => bail!(
            "Profile unit '{}' requires unimplemented plugin type '{other}'",
            unit.name
        ),
    }
}

pub fn cleanup_runtime_resources() {
    eeepc_she::cleanup();
    scheduler::cleanup();
    rtentsk::cleanup();
    cpu::cleanup_latency();
}

fn apply_cpu_unit(rollback: &Rollback, unit: &ProfileUnit) -> Result<()> {
    for (option, value) in &unit.options {
        match option.as_str() {
            "governor" => cpu::apply_governor(rollback, value)?,
            "energy_perf_bias" => cpu::apply_energy_perf_bias(rollback, value)?,
            "energy_performance_preference" => cpu::apply_epp(rollback, value)?,
            "min_perf_pct" => cpu::apply_min_perf_pct(rollback, value)?,
            "max_perf_pct" => cpu::apply_max_perf_pct(rollback, value)?,
            "boost" => cpu::apply_boost(rollback, value)?,
            "no_turbo" => cpu::apply_no_turbo(rollback, value)?,
            "force_latency" => cpu::apply_force_latency(rollback, value)?,
            "pm_qos_resume_latency_us" => cpu::apply_pm_qos_resume_latency_us(rollback, value)?,
            "sampling_down_factor" => cpu::apply_sampling_down_factor(rollback, value)?,
            "load_threshold" | "latency_low" | "latency_high" => {}
            other => bail!(
                "Profile unit '{}' uses unsupported CPU option '{other}'",
                unit.name
            ),
        }
    }
    cpu::apply_dynamic_latency(&unit.options)?;
    Ok(())
}

fn apply_acpi_unit(rollback: &Rollback, unit: &ProfileUnit) -> Result<()> {
    for (option, value) in &unit.options {
        match option.as_str() {
            "platform_profile" => acpi::apply_platform_profile(rollback, value)?,
            other => bail!(
                "Profile unit '{}' uses unsupported ACPI option '{other}'",
                unit.name
            ),
        }
    }
    Ok(())
}

fn validate_unit_contract(unit: &ProfileUnit) -> Result<()> {
    if unit.devices_udev_regex.is_some() {
        bail!(
            "Profile unit '{}' requires devices_udev_regex matching, which is not implemented yet",
            unit.name
        );
    }
    if unit.script_pre.is_some() || unit.script_post.is_some() {
        bail!(
            "Profile unit '{}' requires per-device script_pre/script_post hooks, which are not implemented yet",
            unit.name
        );
    }
    if unit.devices != "*"
        && !matches!(
            unit.plugin_type.as_str(),
            "disk" | "scsi_host" | "usb" | "uncore" | "net" | "network" | "irq" | "mounts"
        )
    {
        bail!(
            "Profile unit '{}' selects devices '{}' for plugin '{}', but that device selector is not implemented yet",
            unit.name,
            unit.devices,
            unit.plugin_type
        );
    }

    if matches!(
        unit.plugin_type.as_str(),
        "modules" | "sysctl" | "sysfs" | "service"
    ) {
        return Ok(());
    }

    let Some(descriptor) = plugins::descriptor(&unit.plugin_type) else {
        bail!(
            "Profile unit '{}' requires unimplemented plugin type '{}'",
            unit.name,
            unit.plugin_type
        );
    };
    for (option, _) in &unit.options {
        if !descriptor
            .options
            .iter()
            .any(|supported| supported.name == option)
        {
            bail!(
                "Profile unit '{}' uses unsupported option '{}.{}'",
                unit.name,
                unit.plugin_type,
                option
            );
        }
    }
    Ok(())
}

fn apply_legacy_projection(rollback: &Rollback, profile: &Profile) -> Result<()> {
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
    disk::apply_options(
        rollback,
        profile.disk.devices.as_deref(),
        &disk_option_pairs(&profile.disk),
    )?;
    if let Some(platform_profile) = &profile.acpi.platform_profile {
        acpi::apply_platform_profile(rollback, platform_profile)?;
    }
    network::apply_tcp_options(rollback, &network_option_pairs(&profile.network))?;
    gpu::apply_gpu_options(rollback, &profile.gpu)?;
    storage::apply_storage_options(rollback, &profile.storage)?;
    thermal::apply_thermal_options(rollback, &profile.thermal)?;
    battery::apply_battery_options(rollback, &profile.battery)?;
    hermes::apply_hermes_options(rollback, &profile.hermes)?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile_units::ProfileUnit;

    #[test]
    fn rejects_unknown_plugins_and_options_before_mutation() {
        let unknown = ProfileUnit::from_options("mystery", Vec::new()).unwrap();
        assert!(validate_unit_contract(&unknown).is_err());

        let cpu =
            ProfileUnit::from_options("cpu", vec![("imaginary".to_string(), "1".to_string())])
                .unwrap();
        assert!(validate_unit_contract(&cpu).is_err());
    }

    #[test]
    fn accepts_dynamic_options_for_modules_sysctl_and_sysfs() {
        let modules = ProfileUnit::from_options(
            "modules",
            vec![("snd_hda_intel".to_string(), "power_save=1".to_string())],
        )
        .unwrap();
        let sysctl = ProfileUnit::from_options(
            "sysctl",
            vec![("vm.swappiness".to_string(), "10".to_string())],
        )
        .unwrap();
        let sysfs = ProfileUnit::from_options(
            "sysfs",
            vec![(
                "/sys/devices/system/machinecheck/machinecheck*/ignore_ce".to_string(),
                "1".to_string(),
            )],
        )
        .unwrap();
        assert!(validate_unit_contract(&modules).is_ok());
        assert!(validate_unit_contract(&sysctl).is_ok());
        assert!(validate_unit_contract(&sysfs).is_ok());
    }
}
