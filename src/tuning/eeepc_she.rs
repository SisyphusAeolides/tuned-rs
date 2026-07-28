use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::config;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;

struct Runtime {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    path: PathBuf,
    original: String,
}

#[derive(Clone, Copy)]
struct Settings {
    normal_threshold: f64,
    powersave_threshold: f64,
    normal_mode: u32,
    powersave_mode: u32,
}

#[derive(Clone, Copy)]
struct CpuSample {
    total: u64,
    idle: u64,
}

static RUNTIME: OnceLock<Mutex<Option<Runtime>>> = OnceLock::new();

pub fn apply(options: &PluginOptions) -> Result<()> {
    cleanup();
    let settings = parse_settings(options)?;
    let Some(path) = control_path() else {
        return Ok(());
    };
    let original = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?
        .trim()
        .to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker_path = path.clone();
    let thread = thread::Builder::new()
        .name("tuned-rs-eeepc-she".to_string())
        .spawn(move || monitor(worker_stop, worker_path, settings))?;
    *runtime_slot().lock().unwrap() = Some(Runtime {
        stop,
        thread,
        path,
        original,
    });
    Ok(())
}

pub fn cleanup() {
    let Some(runtime) = runtime_slot().lock().unwrap().take() else {
        return;
    };
    runtime.stop.store(true, Ordering::Release);
    let _ = runtime.thread.join();
    let _ = fs::write(runtime.path, runtime.original);
}

pub fn verify() -> bool {
    control_path().is_none() || runtime_slot().lock().unwrap().is_some()
}

fn monitor(stop: Arc<AtomicBool>, path: PathBuf, settings: Settings) {
    let mut previous = read_cpu_sample(&config::resolve_path("/proc/stat")).ok();
    let mut active_mode = None;
    while !stop.load(Ordering::Acquire) {
        for _ in 0..10 {
            if stop.load(Ordering::Acquire) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let Ok(current) = read_cpu_sample(&config::resolve_path("/proc/stat")) else {
            continue;
        };
        let load = previous.and_then(|old| cpu_load(old, current));
        previous = Some(current);
        let Some(mode) = load.and_then(|load| {
            if load <= settings.powersave_threshold {
                Some(settings.powersave_mode)
            } else if load >= settings.normal_threshold {
                Some(settings.normal_mode)
            } else {
                None
            }
        }) else {
            continue;
        };
        if active_mode != Some(mode) && fs::write(&path, mode.to_string()).is_ok() {
            active_mode = Some(mode);
        }
    }
}

fn parse_settings(options: &PluginOptions) -> Result<Settings> {
    let normal_threshold = parse_threshold(options, "load_threshold_normal", 0.6)?;
    let powersave_threshold = parse_threshold(options, "load_threshold_powersave", 0.4)?;
    if powersave_threshold > normal_threshold {
        bail!("EeePC powersave threshold must not exceed the normal threshold");
    }
    Ok(Settings {
        normal_threshold,
        powersave_threshold,
        normal_mode: parse_mode(options, "she_normal", 1)?,
        powersave_mode: parse_mode(options, "she_powersave", 2)?,
    })
}

fn parse_threshold(options: &PluginOptions, name: &str, default: f64) -> Result<f64> {
    let value = option_value(options, name)
        .map(str::parse::<f64>)
        .transpose()?
        .unwrap_or(default);
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        bail!("{name} must be between zero and one")
    }
}

fn parse_mode(options: &PluginOptions, name: &str, default: u32) -> Result<u32> {
    Ok(option_value(options, name)
        .map(str::parse::<u32>)
        .transpose()?
        .unwrap_or(default))
}

fn control_path() -> Option<PathBuf> {
    [
        "/sys/devices/platform/eeepc/cpufv",
        "/sys/devices/platform/eeepc-wmi/cpufv",
    ]
    .into_iter()
    .map(config::resolve_path)
    .find(|path| path.is_file())
}

fn read_cpu_sample(path: &Path) -> Result<CpuSample> {
    let contents = fs::read_to_string(path)?;
    let line = contents
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or_else(|| anyhow::anyhow!("/proc/stat has no aggregate CPU row"))?;
    let values = line
        .split_whitespace()
        .skip(1)
        .map(str::parse::<u64>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if values.len() < 5 {
        bail!("/proc/stat aggregate CPU row is incomplete");
    }
    Ok(CpuSample {
        total: values.iter().sum(),
        idle: values[3].saturating_add(values.get(4).copied().unwrap_or(0)),
    })
}

fn cpu_load(previous: CpuSample, current: CpuSample) -> Option<f64> {
    let total = current.total.checked_sub(previous.total)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    (total > 0).then(|| 1.0 - idle.min(total) as f64 / total as f64)
}

fn runtime_slot() -> &'static Mutex<Option<Runtime>> {
    RUNTIME.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates_cpu_load_from_proc_stat_deltas() {
        let old = CpuSample {
            total: 100,
            idle: 40,
        };
        let new = CpuSample {
            total: 200,
            idle: 65,
        };
        assert_eq!(cpu_load(old, new), Some(0.75));
        assert_eq!(cpu_load(new, old), None);
    }

    #[test]
    fn validates_hysteresis_threshold_order() {
        let options = vec![
            ("load_threshold_powersave".to_string(), "0.8".to_string()),
            ("load_threshold_normal".to_string(), "0.2".to_string()),
        ];
        assert!(parse_settings(&options).is_err());
        assert!(parse_settings(&Vec::new()).is_ok());
    }
}
