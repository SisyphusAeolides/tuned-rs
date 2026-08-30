use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const DEFAULT_WINDOW: usize = 128;
const DEFAULT_DT: f64 = 0.05;
const DEFAULT_MANDELBROT_ITERATIONS: u32 = 64;
const MIN_SAMPLES_FOR_LYAPUNOV: usize = 32;

#[derive(Debug, Clone, Copy)]
pub struct ChaosConfig {
    pub enabled: bool,
    pub window: usize,
    pub dt: f64,
    pub mandelbrot_iterations: u32,
}

impl Default for ChaosConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window: DEFAULT_WINDOW,
            dt: DEFAULT_DT,
            mandelbrot_iterations: DEFAULT_MANDELBROT_ITERATIONS,
        }
    }
}

impl ChaosConfig {
    pub fn sanitized(self) -> Self {
        Self {
            enabled: self.enabled,
            window: self.window.clamp(MIN_SAMPLES_FOR_LYAPUNOV, 4096),
            dt: finite_clamp(self.dt, 0.005, 0.5, DEFAULT_DT),
            mandelbrot_iterations: self.mandelbrot_iterations.clamp(16, 1024),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChaosInput {
    /// CPU utilization as a fraction in the range 0..=1.
    pub cpu: f64,
    /// Memory utilization as a fraction in the range 0..=1.
    pub memory: f64,
    /// I/O pressure as a normalized rate in the range 0..=1.
    pub io: f64,
    /// Network pressure as a normalized rate in the range 0..=1.
    pub network: f64,
    /// GPU utilization as a fraction in the range 0..=1.
    pub gpu: f64,
    /// The hottest observed temperature normalized to a 100 C scale.
    pub temperature: f64,
    /// Power draw normalized by the collector in the range 0..=1.
    pub power: f64,
}

impl ChaosInput {
    fn sanitized(self) -> Self {
        Self {
            cpu: unit(self.cpu),
            memory: unit(self.memory),
            io: unit(self.io),
            network: unit(self.network),
            gpu: unit(self.gpu),
            temperature: unit(self.temperature),
            power: unit(self.power),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ChaosMetrics {
    pub enabled: bool,
    pub samples: usize,
    pub workload: f64,
    pub lorenz_activity: f64,
    pub rossler_activity: f64,
    pub logistic_state: f64,
    pub mandelbrot_complexity: f64,
    pub lyapunov_exponent: f64,
    pub duffing_energy: f64,
    pub stability: f64,
    /// Advisory profile pressure in the range -1..=1. It never writes a
    /// profile or a hardware control by itself.
    pub profile_bias: f64,
}

impl ChaosMetrics {
    pub fn recommended_profile(self) -> &'static str {
        if self.profile_bias >= 0.35 {
            "throughput-performance"
        } else if self.profile_bias <= -0.35 {
            "powersave"
        } else {
            "balanced"
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Point3 {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct DuffingState {
    position: f64,
    velocity: f64,
    phase: f64,
}

pub struct ChaosAnalyzer {
    config: ChaosConfig,
    history: VecDeque<f64>,
    lorenz: Point3,
    rossler: Point3,
    duffing: DuffingState,
    logistic: f64,
    last: ChaosMetrics,
}

impl ChaosAnalyzer {
    pub fn new(config: ChaosConfig) -> Self {
        let config = config.sanitized();
        Self {
            history: VecDeque::with_capacity(config.window),
            config,
            lorenz: Point3 {
                x: 0.1,
                ..Point3::default()
            },
            rossler: Point3 {
                x: 0.1,
                z: 0.1,
                ..Point3::default()
            },
            duffing: DuffingState::default(),
            logistic: 0.37,
            last: ChaosMetrics::default(),
        }
    }

    pub fn config(&self) -> ChaosConfig {
        self.config
    }

    pub fn last(&self) -> ChaosMetrics {
        self.last
    }

    pub fn observe(&mut self, input: ChaosInput) -> ChaosMetrics {
        if !self.config.enabled {
            return self.last;
        }

        let input = input.sanitized();
        let workload = (input.cpu * 0.45
            + input.memory * 0.15
            + input.io * 0.15
            + input.network * 0.10
            + input.gpu * 0.10
            + input.power * 0.05)
            .clamp(0.0, 1.0);
        self.history.push_back(workload);
        while self.history.len() > self.config.window {
            self.history.pop_front();
        }

        let previous = self
            .history
            .iter()
            .rev()
            .nth(1)
            .copied()
            .unwrap_or(workload);
        let forcing = (workload - 0.5) + (workload - previous) * 2.0;
        self.step_lorenz(workload, forcing);
        self.step_rossler(workload, forcing);
        self.step_logistic(workload);
        self.step_duffing(workload, forcing);

        let lyapunov = estimate_lyapunov(&self.history, self.config.dt);
        let mandelbrot = mandelbrot_complexity(
            -0.8 + 1.6 * workload,
            -0.8 + 1.6 * input.memory,
            self.config.mandelbrot_iterations,
        );
        let lorenz_activity = point_activity(self.lorenz, 75.0);
        let rossler_activity = point_activity(self.rossler, 50.0);
        let duffing_energy =
            ((0.5 * self.duffing.velocity.powi(2) + 0.25 * self.duffing.position.powi(4)).sqrt()
                / 4.0)
                .clamp(0.0, 1.0);
        let instability = (lyapunov.max(0.0) / 2.0).clamp(0.0, 1.0);
        let stability = (1.0 - instability * 0.7 - input.temperature * 0.3).clamp(0.0, 1.0);
        let pressure = (workload * 0.60
            + instability * 0.20
            + duffing_energy * 0.10
            + (1.0 - stability) * 0.10)
            .clamp(0.0, 1.0);

        self.last = ChaosMetrics {
            enabled: true,
            samples: self.history.len(),
            workload,
            lorenz_activity,
            rossler_activity,
            logistic_state: self.logistic,
            mandelbrot_complexity: mandelbrot,
            lyapunov_exponent: lyapunov,
            duffing_energy,
            stability,
            profile_bias: ((pressure - 0.45) * 2.0).clamp(-1.0, 1.0),
        };
        self.last
    }

    fn step_lorenz(&mut self, workload: f64, forcing: f64) {
        let sigma = 10.0;
        let rho = 28.0 + workload * 2.0;
        let beta = 8.0 / 3.0;
        let dx = sigma * (self.lorenz.y - self.lorenz.x);
        let dy = self.lorenz.x * (rho - self.lorenz.z) - self.lorenz.y + forcing;
        let dz = self.lorenz.x * self.lorenz.y - beta * self.lorenz.z;
        self.lorenz.x = bounded(self.lorenz.x + self.config.dt * dx, -60.0, 60.0);
        self.lorenz.y = bounded(self.lorenz.y + self.config.dt * dy, -60.0, 60.0);
        self.lorenz.z = bounded(self.lorenz.z + self.config.dt * dz, -60.0, 60.0);
    }

    fn step_rossler(&mut self, workload: f64, forcing: f64) {
        let a = 0.2;
        let b = 0.2;
        let c = 5.7 + workload * 0.5;
        let dx = -self.rossler.y - self.rossler.z + forcing * 0.2;
        let dy = self.rossler.x + a * self.rossler.y;
        let dz = b + self.rossler.z * (self.rossler.x - c);
        self.rossler.x = bounded(self.rossler.x + self.config.dt * dx, -50.0, 50.0);
        self.rossler.y = bounded(self.rossler.y + self.config.dt * dy, -50.0, 50.0);
        self.rossler.z = bounded(self.rossler.z + self.config.dt * dz, -50.0, 50.0);
    }

    fn step_logistic(&mut self, workload: f64) {
        let r = 3.55 + workload * 0.4;
        self.logistic =
            (r * self.logistic * (1.0 - self.logistic) + (workload - 0.5) * 0.01).clamp(0.0, 1.0);
    }

    fn step_duffing(&mut self, workload: f64, forcing: f64) {
        let alpha = -1.0;
        let beta = 1.0;
        let damping = 0.25;
        let amplitude = 0.3 + workload * 0.4;
        let drive = amplitude * self.duffing.phase.cos() + forcing * 0.05;
        let acceleration = drive
            - damping * self.duffing.velocity
            - alpha * self.duffing.position
            - beta * self.duffing.position.powi(3);
        self.duffing.velocity = bounded(
            self.duffing.velocity + self.config.dt * acceleration,
            -8.0,
            8.0,
        );
        self.duffing.position = bounded(
            self.duffing.position + self.config.dt * self.duffing.velocity,
            -4.0,
            4.0,
        );
        self.duffing.phase =
            (self.duffing.phase + self.config.dt * 1.2).rem_euclid(std::f64::consts::TAU);
    }
}

fn unit(value: f64) -> f64 {
    finite_clamp(value, 0.0, 1.0, 0.0)
}

fn finite_clamp(value: f64, minimum: f64, maximum: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        fallback.clamp(minimum, maximum)
    }
}

fn bounded(value: f64, minimum: f64, maximum: f64) -> f64 {
    finite_clamp(value, minimum, maximum, 0.0)
}

fn point_activity(point: Point3, scale: f64) -> f64 {
    (point.x.hypot(point.y).hypot(point.z) / scale).clamp(0.0, 1.0)
}

fn mandelbrot_complexity(c_re: f64, c_im: f64, max_iterations: u32) -> f64 {
    let mut z_re = 0.0;
    let mut z_im = 0.0;
    for iteration in 0..max_iterations {
        let next_re = z_re * z_re - z_im * z_im + c_re;
        let next_im = 2.0 * z_re * z_im + c_im;
        z_re = next_re;
        z_im = next_im;
        if z_re * z_re + z_im * z_im > 4.0 {
            return iteration as f64 / max_iterations as f64;
        }
    }
    1.0
}

fn estimate_lyapunov(history: &VecDeque<f64>, dt: f64) -> f64 {
    let values: Vec<f64> = history.iter().copied().collect();
    let length = values.len();
    if length < MIN_SAMPLES_FOR_LYAPUNOV {
        return 0.0;
    }

    let horizon = (length / 16).clamp(1, 8);
    let separation = (length / 10).max(4);
    let max_index = length.saturating_sub(horizon);
    let mut sum = 0.0;
    let mut count = 0_u32;

    for index in 0..max_index {
        let mut nearest = None;
        let mut nearest_distance = f64::INFINITY;
        for candidate in 0..max_index {
            if candidate.abs_diff(index) < separation {
                continue;
            }
            let distance = (values[index] - values[candidate]).abs();
            if distance > 1.0e-5 && distance < nearest_distance {
                nearest = Some(candidate);
                nearest_distance = distance;
            }
        }

        let Some(candidate) = nearest else {
            continue;
        };
        let future_distance = (values[index + horizon] - values[candidate + horizon])
            .abs()
            .max(1.0e-8);
        sum += (future_distance / nearest_distance).ln();
        count += 1;
    }

    if count == 0 {
        0.0
    } else {
        finite_clamp(sum / count as f64 / (horizon as f64 * dt), -5.0, 5.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(load: f64) -> ChaosInput {
        ChaosInput {
            cpu: load,
            memory: load * 0.8,
            io: load * 0.6,
            network: load * 0.4,
            gpu: load * 0.7,
            temperature: load * 0.7,
            power: load * 0.5,
        }
    }

    #[test]
    fn disabled_analyzer_is_inert() {
        let mut analyzer = ChaosAnalyzer::new(ChaosConfig {
            enabled: false,
            ..ChaosConfig::default()
        });
        let metrics = analyzer.observe(input(0.8));
        assert!(!metrics.enabled);
        assert_eq!(metrics.samples, 0);
    }

    #[test]
    fn states_remain_finite_and_bounded() {
        let mut analyzer = ChaosAnalyzer::new(ChaosConfig {
            enabled: true,
            ..ChaosConfig::default()
        });
        for index in 0..512 {
            let load = ((index as f64) * 0.17).sin().mul_add(0.5, 0.5);
            let metrics = analyzer.observe(input(load));
            assert!(metrics.logistic_state.is_finite());
            assert!((0.0..=1.0).contains(&metrics.logistic_state));
            assert!((0.0..=1.0).contains(&metrics.lorenz_activity));
            assert!((0.0..=1.0).contains(&metrics.rossler_activity));
            assert!((0.0..=1.0).contains(&metrics.mandelbrot_complexity));
            assert!((0.0..=1.0).contains(&metrics.duffing_energy));
            assert!((-1.0..=1.0).contains(&metrics.profile_bias));
        }
    }

    #[test]
    fn mandelbrot_score_reflects_escape_depth() {
        let inside = mandelbrot_complexity(0.0, 0.0, 64);
        let outside = mandelbrot_complexity(2.0, 2.0, 64);
        assert!(inside > outside);
    }

    #[test]
    fn lyapunov_estimate_is_finite() {
        let mut history = VecDeque::new();
        for index in 0..128 {
            history.push_back(((index as f64) * 0.31).sin().mul_add(0.5, 0.5));
        }
        let estimate = estimate_lyapunov(&history, DEFAULT_DT);
        assert!(estimate.is_finite());
        assert!((-5.0..=5.0).contains(&estimate));
    }
}
