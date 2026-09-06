use anyhow::Result;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};
use tracing::info;

use crate::chaos::{ChaosAnalyzer, ChaosInput, ChaosMetrics};

pub struct WorkloadDetector {
    last_check: SystemTime,
    check_interval: Duration,
    current_workload: WorkloadType,
    chaos: ChaosAnalyzer,
    last_chaos: ChaosMetrics,
}

#[derive(Debug, Default)]
pub struct AdaptiveState {
    candidate: Option<String>,
    confirmations: u32,
    last_switch: Option<Instant>,
}

impl AdaptiveState {
    pub fn should_switch(
        &mut self,
        active: &str,
        target: &str,
        now: Instant,
        min_dwell: Duration,
        required_confirmations: u32,
    ) -> bool {
        if active == target {
            self.candidate = None;
            self.confirmations = 0;
            return false;
        }

        if self.candidate.as_deref() != Some(target) {
            self.candidate = Some(target.to_string());
            self.confirmations = 1;
        } else {
            self.confirmations = self.confirmations.saturating_add(1);
        }

        if self.confirmations < required_confirmations.max(1) {
            return false;
        }
        self.last_switch
            .map(|last| now.duration_since(last) >= min_dwell)
            .unwrap_or(true)
    }

    pub fn record_switch(&mut self, now: Instant) {
        self.last_switch = Some(now);
        self.candidate = None;
        self.confirmations = 0;
    }

    pub fn record_failure(&mut self) {
        self.candidate = None;
        self.confirmations = 0;
    }
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

impl Default for WorkloadDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkloadDetector {
    pub fn new() -> Self {
        Self {
            last_check: SystemTime::now(),
            check_interval: Duration::from_secs(5),
            current_workload: WorkloadType::Idle,
            chaos: ChaosAnalyzer::new(crate::config::chaos_config()),
            last_chaos: ChaosMetrics::default(),
        }
    }

    pub fn detect_workload(&mut self) -> Result<WorkloadType> {
        let now = SystemTime::now();
        if now
            .duration_since(self.last_check)
            .unwrap_or(Duration::ZERO)
            < self.check_interval
        {
            return Ok(self.current_workload);
        }
        self.last_check = now;

        let cpu_usage = self.get_cpu_usage()?;
        let io_usage = self.get_io_usage()?;
        let gpu_usage = self.get_gpu_usage()?;
        self.last_chaos = self.chaos.observe(ChaosInput {
            cpu: cpu_usage / 100.0,
            memory: 0.0,
            io: io_usage / 100.0,
            network: 0.0,
            gpu: gpu_usage / 100.0,
            temperature: 0.0,
            power: 0.0,
        });

        let workload = match (cpu_usage, io_usage, gpu_usage) {
            (_, _, g) if g > 80.0 => WorkloadType::Gaming,
            (c, _, _) if c > 90.0 => WorkloadType::Compilation,
            (c, i, _) if c > 60.0 || i > 60.0 => WorkloadType::Heavy,
            (c, i, _) if c > 30.0 || i > 30.0 => WorkloadType::Moderate,
            (c, i, _) if c > 10.0 || i > 10.0 => WorkloadType::Light,
            _ => WorkloadType::Idle,
        };

        if workload != self.current_workload {
            info!(
                "Workload changed: {:?} -> {:?}",
                self.current_workload, workload
            );
            self.current_workload = workload;
        }

        Ok(workload)
    }

    fn get_cpu_usage(&self) -> Result<f64> {
        let stat = fs::read_to_string("/proc/stat")?;
        let line = stat.lines().next().unwrap_or("");
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            return Ok(0.0);
        }

        let user: u64 = parts[1].parse().unwrap_or(0);
        let nice: u64 = parts[2].parse().unwrap_or(0);
        let system: u64 = parts[3].parse().unwrap_or(0);
        let idle: u64 = parts[4].parse().unwrap_or(0);

        let total = user + nice + system + idle;
        let active = user + nice + system;

        Ok(if total > 0 {
            (active as f64 / total as f64) * 100.0
        } else {
            0.0
        })
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

    pub fn recommend_profile_with_chaos(&self) -> &str {
        let baseline = self.recommend_profile();
        if !self.last_chaos.enabled || self.last_chaos.samples < 32 {
            return baseline;
        }

        match (baseline, self.last_chaos.recommended_profile()) {
            ("powersave", "throughput-performance") => "balanced",
            ("throughput-performance", "powersave") => "balanced",
            (_, recommendation) => recommendation,
        }
    }

    pub fn chaos_metrics(&self) -> ChaosMetrics {
        self.last_chaos
    }
}

#[cfg(test)]
mod adaptive_tests {
    use super::*;

    #[test]
    fn adaptive_state_requires_confirmation_and_dwell() {
        let start = Instant::now();
        let mut state = AdaptiveState::default();
        let dwell = Duration::from_secs(60);
        assert!(!state.should_switch("balanced", "throughput-performance", start, dwell, 3));
        assert!(!state.should_switch(
            "balanced",
            "throughput-performance",
            start + Duration::from_secs(5),
            dwell,
            3
        ));
        assert!(state.should_switch(
            "balanced",
            "throughput-performance",
            start + Duration::from_secs(10),
            dwell,
            3
        ));
        state.record_switch(start + Duration::from_secs(10));
        assert!(!state.should_switch(
            "throughput-performance",
            "balanced",
            start + Duration::from_secs(20),
            dwell,
            1
        ));
        assert!(state.should_switch(
            "throughput-performance",
            "balanced",
            start + Duration::from_secs(70),
            dwell,
            1
        ));
    }
}
