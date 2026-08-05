use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;
use anyhow::Result;
use std::fs;
use std::path::Path;
use tracing::{debug, error, info, warn};

const THERMAL_BASE: &str = "/sys/class/thermal";
const HWMON_BASE: &str = "/sys/class/hwmon";
const MINIMUM_CPU_TEMP_LIMIT_CELSIUS: u32 = 1;
const MAXIMUM_CPU_TEMP_LIMIT_CELSIUS: u32 = 125;

pub fn apply_thermal_options(rollback: &Rollback, options: &[(String, String)]) -> Result<()> {
    if options.is_empty() {
        return Ok(());
    }
    let mut updated = 0usize;
    for (key, value) in options {
        match apply_thermal_option(rollback, key, value) {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(error) => error!("Failed to apply thermal option {key}={value}: {error}"),
        }
    }
    if updated > 0 {
        info!("Applied {updated} thermal tuning option(s)");
    }
    Ok(())
}

fn apply_thermal_option(rollback: &Rollback, key: &str, value: &str) -> Result<bool> {
    match key {
        "cpu_temp_limit" => apply_cpu_temp_limit(rollback, value),
        "fan_control" => apply_fan_control(rollback, value),
        "thermal_policy" => apply_thermal_policy(rollback, value),
        "trip_point" => apply_trip_point(rollback, value),
        _ => {
            warn!("Unknown thermal option: {key}");
            Ok(false)
        }
    }
}

fn apply_cpu_temp_limit(rollback: &Rollback, value: &str) -> Result<bool> {
    let thermal_path = Path::new(THERMAL_BASE);
    if !thermal_path.exists() {
        return Ok(false);
    }

    for entry in fs::read_dir(thermal_path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("thermal_zone") {
            continue;
        }

        let type_path = entry.path().join("type");
        if !type_path.exists() {
            continue;
        }
        let zone_type = read_trimmed(&type_path)?;
        if !zone_type.contains("cpu") && !zone_type.contains("x86_pkg_temp") {
            continue;
        }

        let trip_path = entry.path().join("trip_point_0_temp");
        if !trip_path.exists() {
            continue;
        }

        let temp_millidegrees = parse_cpu_temp_limit(value)?;
        let original = read_trimmed(&trip_path)?;
        if original == temp_millidegrees.to_string() {
            continue;
        }

        rollback.record_original(
            &rollback_key("sysfs", &trip_path.to_string_lossy()),
            &original,
        )?;
        fs::write(&trip_path, format!("{}\n", temp_millidegrees))?;
        info!("Set CPU thermal limit to {}°C", value);
        return Ok(true);
    }
    Ok(false)
}

fn parse_cpu_temp_limit(value: &str) -> Result<u32> {
    let temperature = value
        .parse::<u32>()
        .map_err(|error| anyhow::anyhow!("invalid CPU temperature limit '{value}': {error}"))?;
    if !(MINIMUM_CPU_TEMP_LIMIT_CELSIUS..=MAXIMUM_CPU_TEMP_LIMIT_CELSIUS).contains(&temperature) {
        anyhow::bail!(
            "CPU temperature limit must be between {MINIMUM_CPU_TEMP_LIMIT_CELSIUS} and \
             {MAXIMUM_CPU_TEMP_LIMIT_CELSIUS} degrees Celsius"
        );
    }
    temperature
        .checked_mul(1000)
        .ok_or_else(|| anyhow::anyhow!("CPU temperature limit overflows millidegrees"))
}

fn apply_fan_control(rollback: &Rollback, value: &str) -> Result<bool> {
    let hwmon_path = Path::new(HWMON_BASE);
    if !hwmon_path.exists() {
        return Ok(false);
    }

    for entry in fs::read_dir(hwmon_path)? {
        let entry = entry?;
        let pwm_enable = entry.path().join("pwm1_enable");
        if !pwm_enable.exists() {
            continue;
        }

        let mode = match value {
            "auto" | "automatic" => "2",
            "manual" | "full" => "1",
            _ => {
                warn!("Unknown fan control mode: {value}");
                continue;
            }
        };

        let original = read_trimmed(&pwm_enable)?;
        if original == mode {
            continue;
        }

        rollback.record_original(
            &rollback_key("sysfs", &pwm_enable.to_string_lossy()),
            &original,
        )?;
        fs::write(&pwm_enable, format!("{}\n", mode))?;
        info!("Set fan control to {value} mode");
        return Ok(true);
    }
    Ok(false)
}

fn apply_thermal_policy(rollback: &Rollback, value: &str) -> Result<bool> {
    let policy_path = Path::new("/sys/devices/virtual/thermal/thermal_zone0/policy");
    if !policy_path.exists() {
        debug!("Thermal policy control not available");
        return Ok(false);
    }

    let original = read_trimmed(policy_path)?;
    if original == value {
        return Ok(false);
    }

    rollback.record_original(
        &rollback_key("sysfs", &policy_path.to_string_lossy()),
        &original,
    )?;
    fs::write(policy_path, format!("{}\n", value))?;
    info!("Set thermal policy to {value}");
    Ok(true)
}

fn apply_trip_point(rollback: &Rollback, value: &str) -> Result<bool> {
    apply_cpu_temp_limit(rollback, value)
}

pub fn get_current_temps() -> Result<Vec<(String, f64)>> {
    let mut temps = Vec::new();
    let thermal_path = Path::new(THERMAL_BASE);
    if !thermal_path.exists() {
        return Ok(temps);
    }

    for entry in fs::read_dir(thermal_path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("thermal_zone") {
            continue;
        }

        let type_path = entry.path().join("type");
        let temp_path = entry.path().join("temp");
        if !type_path.exists() || !temp_path.exists() {
            continue;
        }

        let zone_type = read_trimmed(&type_path)?;
        let temp_str = read_trimmed(&temp_path)?;
        if let Ok(temp_millidegrees) = temp_str.parse::<f64>() {
            temps.push((zone_type, temp_millidegrees / 1000.0));
        }
    }
    Ok(temps)
}

#[cfg(test)]
mod tests {
    use super::parse_cpu_temp_limit;

    #[test]
    fn accepts_bounded_cpu_temperature_limits() {
        assert_eq!(parse_cpu_temp_limit("85").unwrap(), 85_000);
    }

    #[test]
    fn rejects_malformed_and_out_of_range_temperature_limits() {
        for value in ["eighty-five", "0", "126", "4294968"] {
            assert!(parse_cpu_temp_limit(value).is_err(), "{value} should fail");
        }
    }
}
