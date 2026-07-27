pub mod acpi;
pub mod battery;
pub mod cpu;
pub mod disk;
pub mod gpu;
pub mod hermes;
pub mod modifiers;
pub mod network;
pub mod storage;
pub mod sysctl;
pub mod sysfs;
pub mod thermal;
pub mod vm;

use anyhow::Result;

use crate::profile::{DiskSettings, NetworkSettings, Profile, VmSettings};
use crate::rollback::Rollback;

pub fn apply_profile(rollback: &Rollback, profile: &Profile) -> Result<()> {
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
