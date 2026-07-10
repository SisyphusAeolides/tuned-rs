use std::collections::HashMap;
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

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
}

pub struct TelemetryCollector {
    metrics_history: Vec<PerformanceMetrics>,
    max_history: usize,
    last_io_stats: Option<(u64, u64)>,
    last_net_stats: Option<(u64, u64)>,
}

impl TelemetryCollector {
    pub fn new() -> Self {
        Self {
            metrics_history: Vec::new(),
            max_history: 1000,
            last_io_stats: None,
            last_net_stats: None,
        }
    }

    pub fn collect(&mut self) -> Result<PerformanceMetrics> {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        
        let metrics = PerformanceMetrics {
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
        };

        self.metrics_history.push(metrics.clone());
        if self.metrics_history.len() > self.max_history {
            self.metrics_history.remove(0);
        }

        Ok(metrics)
    }

    fn get_cpu_usage(&self) -> Result<f64> {
        let stat = fs::read_to_string("/proc/stat")?;
        let line = stat.lines().next().unwrap_or("");
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 { return Ok(0.0); }
        
        let user: u64 = parts[1].parse().unwrap_or(0);
        let nice: u64 = parts[2].parse().unwrap_or(0);
        let system: u64 = parts[3].parse().unwrap_or(0);
        let idle: u64 = parts[4].parse().unwrap_or(0);
        let iowait: u64 = parts[5].parse().unwrap_or(0);
        
        let total = user + nice + system + idle + iowait;
        let active = user + nice + system;
        
        Ok(if total > 0 { (active as f64 / total as f64) * 100.0 } else { 0.0 })
    }

    fn get_memory_usage(&self) -> Result<f64> {
        let meminfo = fs::read_to_string("/proc/meminfo")?;
        let mut total = 0u64;
        let mut available = 0u64;
        
        for line in meminfo.lines() {
            if line.starts_with("MemTotal:") {
                total = line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
            } else if line.starts_with("MemAvailable:") {
                available = line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
            }
        }
        
        Ok(if total > 0 { ((total - available) as f64 / total as f64) * 100.0 } else { 0.0 })
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
                if !name.starts_with("thermal_zone") { continue; }
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
        if self.metrics_history.is_empty() { return None; }
        
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        let cutoff = now.saturating_sub(duration.as_secs());
        
        let recent: Vec<_> = self.metrics_history.iter()
            .filter(|m| m.timestamp >= cutoff)
            .collect();
        
        if recent.is_empty() { return None; }
        
        let count = recent.len() as f64;
        Some(PerformanceMetrics {
            timestamp: now,
            cpu_usage: recent.iter().map(|m| m.cpu_usage).sum::<f64>() / count,
            memory_usage: recent.iter().map(|m| m.memory_usage).sum::<f64>() / count,
            io_read_bytes: (recent.iter().map(|m| m.io_read_bytes).sum::<u64>() as f64 / count) as u64,
            io_write_bytes: (recent.iter().map(|m| m.io_write_bytes).sum::<u64>() as f64 / count) as u64,
            network_rx_bytes: (recent.iter().map(|m| m.network_rx_bytes).sum::<u64>() as f64 / count) as u64,
            network_tx_bytes: (recent.iter().map(|m| m.network_tx_bytes).sum::<u64>() as f64 / count) as u64,
            gpu_usage: recent.iter().map(|m| m.gpu_usage).sum::<f64>() / count,
            temperatures: HashMap::new(),
            power_consumption: recent.iter().map(|m| m.power_consumption).sum::<f64>() / count,
        })
    }
}
