use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tracing::{debug, error, info, warn};

use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;
use crate::tuning::sysfs::{allowed_sysfs_path, write_raw as write_sysfs_raw};

const CPUFREQ_BASE: &str = "/sys/devices/system/cpu/cpufreq";
const CPU_BASE: &str = "/sys/devices/system/cpu";

struct LatencyState {
    file: fs::File,
    value: i32,
}

static LATENCY: OnceLock<Mutex<Option<LatencyState>>> = OnceLock::new();

struct DynamicLatencyState {
    stop: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}

static DYNAMIC_LATENCY: OnceLock<Mutex<Option<DynamicLatencyState>>> = OnceLock::new();

const ALLOWED_GOVERNORS: &[&str] = &[
    "performance",
    "powersave",
    "ondemand",
    "conservative",
    "schedutil",
    "userspace",
];

const ALLOWED_EPP_VALUES: &[&str] = &[
    "default",
    "performance",
    "balance_performance",
    "balance_power",
    "power",
];

pub fn is_allowed_governor(governor: &str) -> bool {
    ALLOWED_GOVERNORS.contains(&governor)
}

pub fn is_allowed_epp(value: &str) -> bool {
    ALLOWED_EPP_VALUES.contains(&value)
}

pub fn apply_governor(rollback: &Rollback, raw: &str) -> Result<()> {
    let available = read_available_values(CPUFREQ_BASE, "scaling_available_governors")?;
    let Some(governor) = resolve_choice_for_available(raw, &available, is_allowed_governor) else {
        warn!("No supported governor found in '{raw}' for available [{available}]");
        return Ok(());
    };
    write_cpu_file(rollback, &governor, scaling_governor_path)
}

pub fn apply_epp(rollback: &Rollback, raw: &str) -> Result<()> {
    let available =
        read_available_values(CPUFREQ_BASE, "energy_performance_available_preferences")?;
    if available.is_empty() {
        debug!("CPU energy performance preference is not supported on this platform");
        return Ok(());
    }
    let Some(epp) = resolve_choice_for_available(raw, &available, is_allowed_epp) else {
        warn!("No supported EPP value found in '{raw}' for available [{available}]");
        return Ok(());
    };
    write_cpu_file(rollback, &epp, epp_path)
}

pub fn apply_energy_perf_bias(rollback: &Rollback, raw: &str) -> Result<()> {
    let candidates = fallback_values(raw)?;
    let mut targets = cpu_directories()?
        .into_iter()
        .map(|cpu| cpu.join("power/energy_perf_bias"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    targets.sort_unstable();

    if targets.is_empty() {
        debug!("CPU energy_perf_bias is not supported on this platform");
        return Ok(());
    }

    for target in targets {
        let mut applied = false;
        for candidate in &candidates {
            match write_cpu_node(rollback, &target, candidate) {
                Ok(()) => {
                    applied = true;
                    break;
                }
                Err(error) => debug!(
                    "Could not set {} to '{candidate}': {error}",
                    target.display()
                ),
            }
        }
        if !applied {
            warn!(
                "No energy_perf_bias fallback from '{raw}' was accepted by {}",
                target.display()
            );
        }
    }
    Ok(())
}

pub fn apply_min_perf_pct(rollback: &Rollback, raw: &str) -> Result<()> {
    apply_pstate_percentage(rollback, "min_perf_pct", raw)
}

pub fn apply_max_perf_pct(rollback: &Rollback, raw: &str) -> Result<()> {
    apply_pstate_percentage(rollback, "max_perf_pct", raw)
}

pub fn apply_boost(rollback: &Rollback, raw: &str) -> Result<()> {
    let boost = tuned_bool(raw).with_context(|| format!("Invalid CPU boost value '{raw}'"))?;
    let boost_value = if boost { "1" } else { "0" };
    let no_turbo_value = if boost { "0" } else { "1" };
    let mut updated = 0usize;

    for policy in policy_directories()? {
        let target = policy.join("boost");
        if target.is_file() {
            write_cpu_node(rollback, &target, boost_value)?;
            updated += 1;
        }
    }

    let no_turbo = crate::config::resolve_path(CPU_BASE).join("intel_pstate/no_turbo");
    if no_turbo.is_file() {
        write_cpu_node(rollback, &no_turbo, no_turbo_value)?;
        updated += 1;
    }

    if updated == 0 {
        warn!("CPU boost control is not available on this system");
    }
    Ok(())
}

pub fn apply_no_turbo(rollback: &Rollback, raw: &str) -> Result<()> {
    let no_turbo = tuned_bool(raw).with_context(|| format!("Invalid no_turbo value '{raw}'"))?;
    apply_boost(rollback, if no_turbo { "0" } else { "1" })
}

pub fn apply_pm_qos_resume_latency_us(rollback: &Rollback, raw: &str) -> Result<()> {
    validate_scalar(raw)?;
    let mut updated = 0usize;
    for cpu in cpu_directories()? {
        let target = cpu.join("power/pm_qos_resume_latency_us");
        if target.is_file() {
            write_cpu_node(rollback, &target, raw.trim())?;
            updated += 1;
        }
    }
    if updated == 0 {
        debug!("CPU pm_qos_resume_latency_us is not supported on this platform");
    }
    Ok(())
}

pub fn apply_sampling_down_factor(rollback: &Rollback, raw: &str) -> Result<()> {
    let value = raw
        .trim()
        .parse::<u64>()
        .with_context(|| format!("Invalid sampling_down_factor '{raw}'"))?;
    if value == 0 {
        bail!("sampling_down_factor must be greater than zero");
    }

    let mut updated = 0usize;
    let mut governors = HashSet::new();
    for policy in policy_directories()? {
        let governor_path = policy.join("scaling_governor");
        if let Ok(governor) = read_trimmed(&governor_path) {
            governors.insert(governor);
        }
    }
    for governor in governors {
        let target = crate::config::resolve_path(CPUFREQ_BASE)
            .join(governor)
            .join("sampling_down_factor");
        if target.is_file() {
            write_cpu_node(rollback, &target, &value.to_string())?;
            updated += 1;
        }
    }
    if updated == 0 {
        debug!("sampling_down_factor is not available for the active governor");
    }
    Ok(())
}

pub fn apply_force_latency(_rollback: &Rollback, raw: &str) -> Result<()> {
    let Some(value) = resolve_latency(raw)? else {
        cleanup_latency();
        return Ok(());
    };
    let path = crate::config::resolve_path("/dev/cpu_dma_latency");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    file.write_all(&value.to_ne_bytes())
        .with_context(|| format!("Failed to set PM QoS latency through {}", path.display()))?;
    *latency_slot().lock().unwrap() = Some(LatencyState { file, value });
    Ok(())
}

pub fn cleanup_latency() {
    cleanup_dynamic_latency();
    *latency_slot().lock().unwrap() = None;
}

pub fn apply_dynamic_latency(options: &crate::profile::PluginOptions) -> Result<()> {
    use crate::profile_units::option_value;

    cleanup_dynamic_latency();
    if option_value(options, "force_latency").is_some()
        || option_value(options, "pm_qos_resume_latency_us").is_some()
    {
        return Ok(());
    }
    let threshold = option_value(options, "load_threshold")
        .unwrap_or("0.2")
        .trim()
        .parse::<f64>()?;
    if !(0.0..=1.0).contains(&threshold) {
        bail!("CPU load_threshold must be between 0 and 1");
    }
    let low = resolve_latency(option_value(options, "latency_low").unwrap_or("100"))?
        .ok_or_else(|| anyhow::anyhow!("latency_low cannot resolve to None"))?;
    let high = resolve_latency(option_value(options, "latency_high").unwrap_or("1000"))?
        .ok_or_else(|| anyhow::anyhow!("latency_high cannot resolve to None"))?;
    let path = crate::config::resolve_path("/dev/cpu_dma_latency");
    let file = match fs::OpenOptions::new().write(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let stat = crate::config::resolve_path("/proc/stat");
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker = thread::Builder::new()
        .name("tuned-rs-cpu-latency".into())
        .spawn(move || dynamic_latency_loop(file, stat, threshold, low, high, worker_stop))?;
    *dynamic_latency_slot().lock().unwrap() = Some(DynamicLatencyState { stop, worker });
    Ok(())
}

fn cleanup_dynamic_latency() {
    let Some(state) = dynamic_latency_slot().lock().unwrap().take() else {
        return;
    };
    state.stop.store(true, Ordering::Release);
    let _ = state.worker.join();
}

fn dynamic_latency_slot() -> &'static Mutex<Option<DynamicLatencyState>> {
    DYNAMIC_LATENCY.get_or_init(|| Mutex::new(None))
}

fn dynamic_latency_loop(
    mut file: fs::File,
    stat: PathBuf,
    threshold: f64,
    low: i32,
    high: i32,
    stop: Arc<AtomicBool>,
) {
    let mut previous = read_cpu_sample(&stat).ok();
    let mut selected = None;
    while !stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_secs(1));
        let Ok(current) = read_cpu_sample(&stat) else {
            continue;
        };
        let Some(old) = previous.replace(current) else {
            continue;
        };
        let total = current.0.saturating_sub(old.0);
        let idle = current.1.saturating_sub(old.1);
        if total == 0 {
            continue;
        }
        let load = 1.0 - idle as f64 / total as f64;
        let value = if load < threshold { high } else { low };
        if selected != Some(value) && file.write_all(&value.to_ne_bytes()).is_ok() {
            selected = Some(value);
        }
    }
}

fn read_cpu_sample(path: &Path) -> Result<(u64, u64)> {
    let contents = fs::read_to_string(path)?;
    let fields = contents
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing aggregate CPU statistics"))?
        .split_whitespace()
        .skip(1)
        .map(str::parse::<u64>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if fields.len() < 4 {
        bail!("Incomplete aggregate CPU statistics");
    }
    let total = fields.iter().copied().sum();
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Ok((total, idle))
}

pub fn verify_force_latency(raw: &str) -> bool {
    let Ok(expected) = resolve_latency(raw) else {
        return false;
    };
    let held = latency_slot().lock().unwrap();
    match (expected, held.as_ref()) {
        (None, None) => true,
        (Some(expected), Some(state)) => {
            let _ = &state.file;
            state.value == expected
        }
        _ => false,
    }
}

fn latency_slot() -> &'static Mutex<Option<LatencyState>> {
    LATENCY.get_or_init(|| Mutex::new(None))
}

fn resolve_latency(raw: &str) -> Result<Option<i32>> {
    for candidate in raw.split('|').map(str::trim) {
        if candidate.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        if let Ok(value) = candidate.parse::<i32>() {
            if value < 0 {
                bail!("CPU DMA latency cannot be negative");
            }
            return Ok(Some(value));
        }
        let (kind, key, no_zero) = if let Some(key) = candidate.strip_prefix("cstate.id:") {
            ("id", key, false)
        } else if let Some(key) = candidate.strip_prefix("cstate.id_no_zero:") {
            ("id", key, true)
        } else if let Some(key) = candidate.strip_prefix("cstate.name:") {
            ("name", key, false)
        } else if let Some(key) = candidate.strip_prefix("cstate.name_no_zero:") {
            ("name", key, true)
        } else {
            continue;
        };
        if let Some(value) = cstate_latency(kind, key)? {
            if !no_zero || value != 0 {
                return Ok(Some(value));
            }
        }
    }
    bail!("No CPU latency fallback could be resolved from '{raw}'")
}

fn cstate_latency(kind: &str, key: &str) -> Result<Option<i32>> {
    let root = crate::config::resolve_path("/sys/devices/system/cpu/cpu0/cpuidle");
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let matched = if kind == "id" {
            key.parse::<u32>()
                .ok()
                .is_some_and(|id| name == format!("state{id}"))
        } else {
            fs::read_to_string(entry.path().join("name")).is_ok_and(|name| name.trim() == key)
        };
        if matched {
            let latency = fs::read_to_string(entry.path().join("latency"))?;
            return Ok(Some(latency.trim().parse::<i32>()?));
        }
    }
    Ok(None)
}

fn apply_pstate_percentage(rollback: &Rollback, leaf: &str, raw: &str) -> Result<()> {
    let value = raw
        .trim()
        .parse::<u8>()
        .with_context(|| format!("Invalid {leaf} value '{raw}'"))?;
    if value > 100 {
        bail!("{leaf} must be in the range 0..=100");
    }

    let mut updated = 0usize;
    for driver in ["intel_pstate", "amd_pstate"] {
        let target = crate::config::resolve_path(CPU_BASE)
            .join(driver)
            .join(leaf);
        if target.is_file() {
            write_cpu_node(rollback, &target, &value.to_string())?;
            updated += 1;
        }
    }
    if updated == 0 {
        debug!("{leaf} is not supported by an active P-state driver");
    }
    Ok(())
}

fn resolve_choice_for_available(
    raw: &str,
    available: &str,
    is_valid: impl Fn(&str) -> bool,
) -> Option<String> {
    let available: HashSet<&str> = available.split_whitespace().collect();
    for candidate in raw
        .split('|')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if is_valid(candidate) && available.contains(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn fallback_values(raw: &str) -> Result<Vec<String>> {
    let candidates = raw
        .split('|')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        bail!("At least one CPU fallback value is required");
    }
    for candidate in &candidates {
        validate_scalar(candidate)?;
    }
    Ok(candidates)
}

fn read_available_values(base: &str, leaf: &str) -> Result<String> {
    for policy in policy_directories_in(&crate::config::resolve_path(base))? {
        let path = policy.join(leaf);
        if path.is_file() {
            return read_trimmed(&path);
        }
    }
    Ok(String::new())
}

fn policy_directories() -> Result<Vec<PathBuf>> {
    policy_directories_in(&crate::config::resolve_path(CPUFREQ_BASE))
}

fn policy_directories_in(base: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut policies = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_prefix("policy")
                .is_some_and(|suffix| {
                    !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
                })
                .then(|| entry.path())
        })
        .collect::<Vec<_>>();
    policies.sort_unstable();
    Ok(policies)
}

fn cpu_directories() -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(crate::config::resolve_path(CPU_BASE)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut cpus = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_prefix("cpu")
                .is_some_and(|suffix| {
                    !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
                })
                .then(|| entry.path())
        })
        .collect::<Vec<_>>();
    cpus.sort_unstable();
    Ok(cpus)
}

fn scaling_governor_path(entry: &fs::DirEntry) -> PathBuf {
    entry.path().join("scaling_governor")
}

fn epp_path(entry: &fs::DirEntry) -> PathBuf {
    entry.path().join("energy_performance_preference")
}

fn write_cpu_file(
    rollback: &Rollback,
    value: &str,
    path_for_entry: fn(&fs::DirEntry) -> PathBuf,
) -> Result<()> {
    let updated = write_file_dir(rollback, CPUFREQ_BASE, value, path_for_entry)?;
    if updated == 0 {
        warn!("No CPU tuning nodes were updated");
    } else {
        info!("Updated CPU settings on {updated} node(s)");
    }
    Ok(())
}

fn write_file_dir(
    rollback: &Rollback,
    base: &str,
    value: &str,
    path_for_entry: fn(&fs::DirEntry) -> PathBuf,
) -> Result<usize> {
    let base_path = crate::config::resolve_path(base);
    let entries = match fs::read_dir(&base_path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };

    let mut updated = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("policy") || !name[6..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let target = path_for_entry(&entry);
        if !target.exists() {
            continue;
        }

        match write_cpu_node(rollback, &target, value) {
            Ok(()) => updated += 1,
            Err(error) => error!("Failed to write {} for {name}: {error}", target.display()),
        }
    }

    Ok(updated)
}

fn write_cpu_node(rollback: &Rollback, target: &Path, value: &str) -> Result<()> {
    validate_cpu_payload(target, value)?;
    let path = allowed_sysfs_path(target)?;
    let original = read_trimmed(&path)?;
    if active_value(&original) == value {
        debug!("Keeping CPU setting {} at '{value}'", path.display());
        return Ok(());
    }
    rollback.record_original(&rollback_key("sysfs", &path.to_string_lossy()), &original)?;
    write_sysfs_raw(&path, value)
}

fn validate_cpu_payload(path: &Path, payload: &str) -> Result<()> {
    validate_scalar(payload)?;
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    match leaf {
        "scaling_governor" if !is_allowed_governor(payload) => {
            bail!("Unknown CPU governor: {payload}")
        }
        "energy_performance_preference" if !is_allowed_epp(payload) => {
            bail!("Unknown energy performance preference: {payload}")
        }
        _ => {}
    }
    Ok(())
}

fn validate_scalar(payload: &str) -> Result<()> {
    if payload.is_empty() || payload.len() > 128 {
        bail!("Invalid CPU control value");
    }
    if payload
        .chars()
        .any(|character| character == '\n' || character == '\0')
    {
        bail!("CPU control values must not contain control characters");
    }
    Ok(())
}

fn tuned_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "y" | "yes" | "t" | "true" | "on" => Some(true),
        "0" | "n" | "no" | "f" | "false" | "off" => Some(false),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolves_numeric_and_cstate_latency_fallbacks() {
        let _env_guard = crate::config::test_env_lock();
        let root = TempDir::new().unwrap();
        let state = root
            .path()
            .join("sys/devices/system/cpu/cpu0/cpuidle/state2");
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("name"), "C2\n").unwrap();
        fs::write(state.join("latency"), "25\n").unwrap();
        std::env::set_var("TUNED_RS_ROOT", root.path());
        assert_eq!(resolve_latency("cstate.name:C2|10").unwrap(), Some(25));
        assert_eq!(resolve_latency("cstate.id:2|10").unwrap(), Some(25));
        assert_eq!(resolve_latency("cstate.name:missing|10").unwrap(), Some(10));
        assert_eq!(resolve_latency("cstate.name:missing|None").unwrap(), None);
        std::env::remove_var("TUNED_RS_ROOT");
    }

    #[test]
    fn governor_apply_and_rollback_stay_below_the_resolved_sysfs_root() {
        let _env_guard = crate::config::test_env_lock();
        let root = TempDir::new().unwrap();
        let policy = root.path().join("sys/devices/system/cpu/cpufreq/policy0");
        fs::create_dir_all(&policy).unwrap();
        fs::write(
            policy.join("scaling_available_governors"),
            "performance powersave",
        )
        .unwrap();
        fs::write(policy.join("scaling_governor"), "powersave").unwrap();
        std::env::set_var("TUNED_RS_ROOT", root.path());

        let rollback = Rollback::load().unwrap();
        apply_governor(&rollback, "performance").unwrap();
        assert_eq!(
            fs::read_to_string(policy.join("scaling_governor")).unwrap(),
            "performance"
        );
        rollback.restore_all().unwrap();
        assert_eq!(
            fs::read_to_string(policy.join("scaling_governor")).unwrap(),
            "powersave"
        );

        std::env::remove_var("TUNED_RS_ROOT");
    }

    #[test]
    fn computes_dynamic_cpu_load_from_stat_deltas() {
        let old = (100_u64, 80_u64);
        let current = (200_u64, 120_u64);
        let load = 1.0 - (current.1 - old.1) as f64 / (current.0 - old.0) as f64;
        assert!((load - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn picks_first_available_governor_from_pipe_list() {
        let chosen = resolve_choice_for_available(
            "schedutil|ondemand|powersave",
            "performance powersave",
            is_allowed_governor,
        );
        assert_eq!(chosen.as_deref(), Some("powersave"));
    }

    #[test]
    fn skips_unavailable_epp_values() {
        let chosen = resolve_choice_for_available(
            "balance_performance|balance_power",
            "default performance balance_performance balance_power power",
            is_allowed_epp,
        );
        assert_eq!(chosen.as_deref(), Some("balance_performance"));
    }

    #[test]
    fn legacy_resolve_choice_still_validates_allowlist() {
        use crate::tuning::modifiers::resolve_choice;

        assert_eq!(
            resolve_choice("schedutil|ondemand", is_allowed_governor).as_deref(),
            Some("schedutil")
        );
    }

    #[test]
    fn normalizes_tuned_boolean_values() {
        assert_eq!(tuned_bool("true"), Some(true));
        assert_eq!(tuned_bool("0"), Some(false));
        assert_eq!(tuned_bool("maybe"), None);
    }

    #[test]
    fn reads_bracket_selected_values() {
        assert_eq!(
            active_value("powersave [performance] schedutil"),
            "performance"
        );
        assert_eq!(active_value("normal"), "normal");
    }

    #[test]
    fn parses_ordered_fallback_values() {
        assert_eq!(
            fallback_values("powersave | power").unwrap(),
            vec!["powersave", "power"]
        );
        assert!(fallback_values(" | ").is_err());
    }
}
