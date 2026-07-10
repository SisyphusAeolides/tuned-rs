use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};
use anyhow::{Context, Result};
use tracing::{debug, info, warn};

pub struct WorkloadDetector {
    last_check: SystemTime,
    check_interval: Duration,
    current_workload: WorkloadType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WorkloadType {
    Idle,
    Light,
    Moderate,
    Heavy,
    Gaming,
    Compilation,
}

impl WorkloadDetector {
    pub fn new() -> Self {
        Self {
            last_check: SystemTime::now(),
            check_interval: Duration::from_secs(5),
            current_workload: WorkloadType::Idle,
        }
    }

    pub fn detect_workload(&mut self) -> Result<WorkloadType> {
        let now = SystemTime::now();
        if now.duration_since(self.last_check).unwrap_or(Duration::ZERO) < self.check_interval {
            return Ok(self.current_workload);
        }
        self.last_check = now;

        let cpu_usage = self.get_cpu_usage()?;
        let io_usage = self.get_io_usage()?;
        let gpu_usage = self.get_gpu_usage()?;

        let workload = match (cpu_usage, io_usage, gpu_usage) {
            (c, _, g) if g > 80.0 => WorkloadType::Gaming,
            (c, _, _) if c > 90.0 => WorkloadType::Compilation,
            (c, i, _) if c > 60.0 || i > 60.0 => WorkloadType::Heavy,
            (c, i, _) if c > 30.0 || i > 30.0 => WorkloadType::Moderate,
            (c, i, _) if c > 10.0 || i > 10.0 => WorkloadType::Light,
            _ => WorkloadType::Idle,
        };

        if workload != self.current_workload {
            info!("Workload changed: {:?} -> {:?}", self.current_workload, workload);
            self.current_workload = workload;
        }

        Ok(workload)
    }

    fn get_cpu_usage(&self) -> Result<f64> {
        let stat = fs::read_to_string("/proc/stat")?;
        let line = stat.lines().next().unwrap_or("");
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 { return Ok(0.0); }
        
        let user: u64 = parts[1].parse().unwrap_or(0);
        let nice: u64 = parts[2].parse().unwrap_or(0);
        let system: u64 = parts[3].parse().unwrap_or(0);
        let idle: u64 = parts[4].parse().unwrap_or(0);
        
        let total = user + nice + system + idle;
        let active = user + nice + system;
        
        Ok(if total > 0 { (active as f64 / total as f64) * 100.0 } else { 0.0 })
    }

    fn get_io_usage(&self) -> Result<f64> {
        let diskstats = fs::read_to_string("/proc/diskstats").unwrap_or_default();
        let mut total_io = 0u64;
        for line in diskstats.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 9 {
                total_io += parts[5].parse::<u64>().unwrap_or(0);
                total_io += parts[9].parse::<u64>().unwrap_or(0);
            }
        }
        Ok((total_io as f64 / 1000.0).min(100.0))
    }

    fn get_gpu_usage(&self) -> Result<f64> {
        let amd_path = Path::new("/sys/class/drm/card0/device/gpu_busy_percent");
        if amd_path.exists() {
            if let Ok(content) = fs::read_to_string(amd_path) {
                return Ok(content.trim().parse().unwrap_or(0.0));
            }
        }
        Ok(0.0)
    }

    pub fn recommend_profile(&self) -> &str {
        match self.current_workload {
            WorkloadType::Idle => "powersave",
            WorkloadType::Light => "balanced",
            WorkloadType::Moderate => "balanced",
            WorkloadType::Heavy => "throughput-performance",
            WorkloadType::Gaming => "latency-performance",
            WorkloadType::Compilation => "throughput-performance",
        }
    }
}
