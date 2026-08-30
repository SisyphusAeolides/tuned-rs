use crate::chaos::{ChaosAnalyzer, ChaosConfig, ChaosInput, ChaosMetrics};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub timestamp: u64,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub gpu_usage: f64,
    pub temperatures: HashMap<String, f64>,
    pub power_consumption: f64,
    #[serde(default)]
    pub chaos: ChaosMetrics,
}

pub struct TelemetryCollector {
    metrics_history: Vec<PerformanceMetrics>,
    max_history: usize,
    last_cpu: Option<(u64, u64)>,
    chaos: ChaosAnalyzer,
    last_counters: Option<(u64, u64, u64, u64)>,
    last_counter_at: Option<Instant>,
}

impl Default for TelemetryCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl TelemetryCollector {
    pub fn new() -> Self {
        Self::with_chaos(crate::config::chaos_config())
    }

    pub fn with_chaos(chaos_config: ChaosConfig) -> Self {
        Self {
            metrics_history: Vec::new(),
            max_history: 1000,
            last_cpu: None,
            chaos: ChaosAnalyzer::new(chaos_config),
            last_counters: None,
            last_counter_at: None,
        }
    }

    pub fn collect(&mut self) -> Result<PerformanceMetrics> {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let mut metrics = PerformanceMetrics {
            timestamp,
            cpu_usage: self.get_cpu_usage()?,
            memory_usage: self.get_memory_usage()?,
            io_read_bytes: self.get_io_read()?,
            io_write_bytes: self.get_io_write()?,
            network_rx_bytes: self.get_net_rx()?,
            network_tx_bytes: self.get_net_tx()?,
            gpu_usage: self.get_gpu_usage()?,
            temperatures: self.get_temperatures()?,
            power_consumption: self.get_power_consumption()?,
            chaos: ChaosMetrics::default(),
        };

        let now = Instant::now();
        let (io, network) = self.pressure_rates(&metrics, now);
        let hottest_temperature = metrics
            .temperatures
            .values()
            .copied()
            .fold(0.0_f64, f64::max)
            .max(0.0)
            / 100.0;
        metrics.chaos = self.chaos.observe(ChaosInput {
            cpu: metrics.cpu_usage / 100.0,
            memory: metrics.memory_usage / 100.0,
            io,
            network,
            gpu: metrics.gpu_usage / 100.0,
            temperature: hottest_temperature,
            power: metrics.power_consumption / 100.0,
        });

        self.metrics_history.push(metrics.clone());
        if self.metrics_history.len() > self.max_history {
            self.metrics_history.remove(0);
        }

        Ok(metrics)
    }

    fn pressure_rates(&mut self, metrics: &PerformanceMetrics, now: Instant) -> (f64, f64) {
        let current = (
            metrics.io_read_bytes,
            metrics.io_write_bytes,
            metrics.network_rx_bytes,
            metrics.network_tx_bytes,
        );
        let rates = match (self.last_counters, self.last_counter_at) {
            (Some(previous), Some(previous_at)) => {
                let seconds = previous_at.elapsed().as_secs_f64().max(0.001);
                let io_bytes = current
                    .0
                    .saturating_sub(previous.0)
                    .saturating_add(current.1.saturating_sub(previous.1));
                let network_bytes = current
                    .2
                    .saturating_sub(previous.2)
                    .saturating_add(current.3.saturating_sub(previous.3));
                (
                    (io_bytes as f64 / (seconds * 50_000_000.0)).clamp(0.0, 1.0),
                    (network_bytes as f64 / (seconds * 10_000_000.0)).clamp(0.0, 1.0),
                )
            }
            _ => (0.0, 0.0),
        };
        self.last_counters = Some(current);
        self.last_counter_at = Some(now);
        rates
    }

    fn get_cpu_usage(&mut self) -> Result<f64> {
        let stat = fs::read_to_string("/proc/stat")?;
        let line = stat.lines().next().unwrap_or("");
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 {
            return Ok(0.0);
        }

        let user: u64 = parts[1].parse().unwrap_or(0);
        let nice: u64 = parts[2].parse().unwrap_or(0);
        let system: u64 = parts[3].parse().unwrap_or(0);
        let idle: u64 = parts[4].parse().unwrap_or(0);
        let iowait: u64 = parts[5].parse().unwrap_or(0);

        let active = user + nice + system;
        let total = active + idle + iowait;
        let previous = self.last_cpu.replace((total, active));
        let Some((old_total, old_active)) = previous else {
            return Ok(if total > 0 {
                active as f64 / total as f64 * 100.0
            } else {
                0.0
            });
        };
        let total_delta = total.saturating_sub(old_total);
        let active_delta = active.saturating_sub(old_active);
        Ok(if total_delta > 0 {
            active_delta as f64 / total_delta as f64 * 100.0
        } else {
            0.0
        })
    }

    fn get_memory_usage(&self) -> Result<f64> {
        let meminfo = fs::read_to_string("/proc/meminfo")?;
        let mut total = 0u64;
        let mut available = 0u64;

        for line in meminfo.lines() {
            if line.starts_with("MemTotal:") {
                total = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
            } else if line.starts_with("MemAvailable:") {
                available = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
            }
        }

        Ok(if total > 0 {
            ((total - available) as f64 / total as f64) * 100.0
        } else {
            0.0
        })
    }

    fn get_io_read(&mut self) -> Result<u64> {
        let diskstats = fs::read_to_string("/proc/diskstats")?;
        let mut total_read = 0u64;
        for line in diskstats.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 5 {
                total_read += parts[5].parse::<u64>().unwrap_or(0) * 512;
            }
        }
        Ok(total_read)
    }

    fn get_io_write(&mut self) -> Result<u64> {
        let diskstats = fs::read_to_string("/proc/diskstats")?;
        let mut total_write = 0u64;
        for line in diskstats.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 9 {
                total_write += parts[9].parse::<u64>().unwrap_or(0) * 512;
            }
        }
        Ok(total_write)
    }

    fn get_net_rx(&mut self) -> Result<u64> {
        let netdev = fs::read_to_string("/proc/net/dev")?;
        let mut total_rx = 0u64;
        for line in netdev.lines().skip(2) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 1 {
                total_rx += parts[1].parse::<u64>().unwrap_or(0);
            }
        }
        Ok(total_rx)
    }

    fn get_net_tx(&mut self) -> Result<u64> {
        let netdev = fs::read_to_string("/proc/net/dev")?;
        let mut total_tx = 0u64;
        for line in netdev.lines().skip(2) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 9 {
                total_tx += parts[9].parse::<u64>().unwrap_or(0);
            }
        }
        Ok(total_tx)
    }

    fn get_gpu_usage(&self) -> Result<f64> {
        if let Ok(content) = fs::read_to_string("/sys/class/drm/card0/device/gpu_busy_percent") {
            return Ok(content.trim().parse().unwrap_or(0.0));
        }
        Ok(0.0)
    }

    fn get_temperatures(&self) -> Result<HashMap<String, f64>> {
        let mut temps = HashMap::new();
        if let Ok(entries) = fs::read_dir("/sys/class/thermal") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with("thermal_zone") {
                    continue;
                }
                if let Ok(temp_str) = fs::read_to_string(entry.path().join("temp")) {
                    if let Ok(temp) = temp_str.trim().parse::<f64>() {
                        temps.insert(name, temp / 1000.0);
                    }
                }
            }
        }
        Ok(temps)
    }

    fn get_power_consumption(&self) -> Result<f64> {
        if let Ok(content) = fs::read_to_string("/sys/class/power_supply/BAT0/power_now") {
            return Ok(content.trim().parse::<f64>().unwrap_or(0.0) / 1_000_000.0);
        }
        Ok(0.0)
    }

    pub fn get_average_metrics(&self, duration: Duration) -> Option<PerformanceMetrics> {
        if self.metrics_history.is_empty() {
            return None;
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        let cutoff = now.saturating_sub(duration.as_secs());

        let recent: Vec<_> = self
            .metrics_history
            .iter()
            .filter(|m| m.timestamp >= cutoff)
            .collect();

        if recent.is_empty() {
            return None;
        }

        let count = recent.len() as f64;
        let chaos = ChaosMetrics {
            enabled: recent.iter().any(|metrics| metrics.chaos.enabled),
            samples: recent
                .last()
                .map(|metrics| metrics.chaos.samples)
                .unwrap_or_default(),
            workload: recent.iter().map(|m| m.chaos.workload).sum::<f64>() / count,
            lorenz_activity: recent.iter().map(|m| m.chaos.lorenz_activity).sum::<f64>() / count,
            rossler_activity: recent.iter().map(|m| m.chaos.rossler_activity).sum::<f64>() / count,
            logistic_state: recent.iter().map(|m| m.chaos.logistic_state).sum::<f64>() / count,
            mandelbrot_complexity: recent
                .iter()
                .map(|m| m.chaos.mandelbrot_complexity)
                .sum::<f64>()
                / count,
            lyapunov_exponent: recent
                .iter()
                .map(|m| m.chaos.lyapunov_exponent)
                .sum::<f64>()
                / count,
            duffing_energy: recent.iter().map(|m| m.chaos.duffing_energy).sum::<f64>() / count,
            stability: recent.iter().map(|m| m.chaos.stability).sum::<f64>() / count,
            profile_bias: recent.iter().map(|m| m.chaos.profile_bias).sum::<f64>() / count,
        };
        Some(PerformanceMetrics {
            timestamp: now,
            cpu_usage: recent.iter().map(|m| m.cpu_usage).sum::<f64>() / count,
            memory_usage: recent.iter().map(|m| m.memory_usage).sum::<f64>() / count,
            io_read_bytes: (recent.iter().map(|m| m.io_read_bytes).sum::<u64>() as f64 / count)
                as u64,
            io_write_bytes: (recent.iter().map(|m| m.io_write_bytes).sum::<u64>() as f64 / count)
                as u64,
            network_rx_bytes: (recent.iter().map(|m| m.network_rx_bytes).sum::<u64>() as f64
                / count) as u64,
            network_tx_bytes: (recent.iter().map(|m| m.network_tx_bytes).sum::<u64>() as f64
                / count) as u64,
            gpu_usage: recent.iter().map(|m| m.gpu_usage).sum::<f64>() / count,
            temperatures: HashMap::new(),
            power_consumption: recent.iter().map(|m| m.power_consumption).sum::<f64>() / count,
            chaos,
        })
    }
}
