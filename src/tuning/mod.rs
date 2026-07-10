pub mod acpi;
pub mod cpu;
pub mod disk;
pub mod modifiers;
pub mod sysctl;
pub mod network;
pub mod gpu;
pub mod storage;
pub mod thermal;
pub mod battery;
pub mod hermes;
pub mod sysfs;
pub mod vm;

use anyhow::Result;

use crate::profile::{DiskSettings, NetworkSettings, Profile, VmSettings};
use crate::rollback::Rollback;

pub fn apply_profile(rollback: &Rollback, profile: &Profile) -> Result<()> {
    if let Some(governor) = &profile.cpu.governor {
        cpu::apply_governor(rollback, governor)?;
    }
    if let Some(epp) = &profile.cpu.energy_performance_preference {
        cpu::apply_epp(rollback, epp)?;
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

    network::apply_tcp_options(rollback, &network_option_pairs(&profile.network))?;
        acpi::apply_platform_profile(rollback, platform_profile)?;

    network::apply_tcp_options(rollback, &network_option_pairs(&profile.network))?;
    }

    network::apply_tcp_options(rollback, &network_option_pairs(&profile.network))?;

    Ok(())
}

fn vm_option_pairs(vm: &VmSettings) -> Vec<(String, String)> {
    let mut options = Vec::new();
    push_option(&mut options, "transparent_hugepages", &vm.transparent_hugepages);
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
    push_option(&mut options, "tcp_congestion_control", &network.tcp_congestion_control);
    push_option(&mut options, "tcp_window_scaling", &network.tcp_window_scaling);
    push_option(&mut options, "tcp_timestamps", &network.tcp_timestamps);
    push_option(&mut options, "tcp_sack", &network.tcp_sack);
    push_option(&mut options, "tcp_fastopen", &network.tcp_fastopen);
    options
}
