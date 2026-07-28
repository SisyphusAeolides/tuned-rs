use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::process::Command;

use anyhow::{bail, Context, Result};
use regex::Regex;
use tracing::info;

use crate::config;

pub fn evaluate(invocation: &str) -> Result<String> {
    let mut fields = invocation.split(':');
    let name = fields.next().unwrap_or_default();
    let args = fields.collect::<Vec<_>>();
    match name {
        "assertion" => {
            min_args(name, &args, 3)?;
            let left = args[1..args.len() - 1].join(":");
            let right = args[args.len() - 1];
            if left != right {
                bail!("Assertion '{}' failed: '{}' != '{}'", args[0], left, right);
            }
            Ok(String::new())
        }
        "assertion_non_equal" => {
            min_args(name, &args, 3)?;
            let left = args[1..args.len() - 1].join(":");
            let right = args[args.len() - 1];
            if left == right {
                bail!("Assertion '{}' failed: values are equal", args[0]);
            }
            Ok(String::new())
        }
        "calc_isolated_cores" => calc_isolated_cores(&args),
        "check_net_queue_count" => {
            exact_args(name, &args, 1)?;
            if args[0].parse::<u32>().is_ok() {
                Ok(args[0].to_string())
            } else {
                Ok(std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(1)
                    .to_string())
            }
        }
        "cpuinfo_check" => match_pairs(&args, &read("/proc/cpuinfo")?),
        "lscpu_check" => {
            let output = Command::new("lscpu")
                .output()
                .context("Failed to execute lscpu")?;
            match_pairs(&args, &String::from_utf8_lossy(&output.stdout))
        }
        "cpulist2devs" => Ok(parse_cpu_list(&args.join(",,"))?
            .into_iter()
            .map(|cpu| format!("cpu{cpu}"))
            .collect::<Vec<_>>()
            .join(",")),
        "cpulist2hex" => cpu_list_to_hex(&parse_cpu_list(&args.join(",,"))?),
        "cpulist2hex_invert" => {
            let inverted = invert_cpu_list(&parse_cpu_list(&args.join(",,"))?)?;
            cpu_list_to_hex(&inverted)
        }
        "cpulist_invert" => Ok(join_cpus(&invert_cpu_list(&parse_cpu_list(
            &args.join(",,"),
        )?)?)),
        "cpulist_online" => {
            let cpus = parse_cpu_list(&args.join(","))?;
            let online = parse_cpu_list(&read("/sys/devices/system/cpu/online")?)?
                .into_iter()
                .collect::<BTreeSet<_>>();
            Ok(join_cpus(
                &cpus
                    .into_iter()
                    .filter(|cpu| online.contains(cpu))
                    .collect::<Vec<_>>(),
            ))
        }
        "cpulist_present" => {
            let cpus = parse_cpu_list(&args.join(",,"))?;
            let present = parse_cpu_list(&read("/sys/devices/system/cpu/present")?)?
                .into_iter()
                .collect::<BTreeSet<_>>();
            Ok(join_cpus(
                &cpus
                    .into_iter()
                    .filter(|cpu| present.contains(cpu))
                    .collect::<Vec<_>>(),
            ))
        }
        "cpulist_pack" => Ok(pack_cpu_list(&parse_cpu_list(&args.join(",,"))?)),
        "cpulist_unpack" => Ok(join_cpus(&parse_cpu_list(&args.join(",,"))?)),
        "exec" => execute(&args),
        "hex2cpulist" => {
            exact_args(name, &args, 1)?;
            Ok(join_cpus(&hex_to_cpu_list(args[0])?))
        }
        "intel_recommended_pstate" => {
            let processor = read("/sys/devices/cpu/caps/pmu_name").unwrap_or_default();
            Ok(if processor.trim().is_empty()
                || [
                    "sandybridge",
                    "ivybridge",
                    "haswell",
                    "broadwell",
                    "skylake",
                ]
                .contains(&processor.trim())
            {
                "disable"
            } else {
                "active"
            }
            .to_string())
        }
        "kb2s" => convert_units(name, &args, |value| value.checked_mul(2)),
        "s2kb" => convert_units(name, &args, |value| {
            let lower = value.div_euclid(2);
            Some(if value.rem_euclid(2) == 0 || lower % 2 == 0 {
                lower
            } else {
                lower + 1
            })
        }),
        "log" => {
            min_args(name, &args, 1)?;
            let value = args.concat();
            info!("TuneD profile: {value}");
            Ok(value)
        }
        "package2cpus" => package_devices(&args, false),
        "package2uncores" => package_devices(&args, true),
        "regex_search_ternary" => {
            exact_args(name, &args, 4)?;
            Ok(if Regex::new(args[1])?.is_match(args[0]) {
                args[2]
            } else {
                args[3]
            }
            .to_string())
        }
        "strip" => {
            min_args(name, &args, 1)?;
            Ok(args.concat().trim().to_string())
        }
        "virt_check" => {
            exact_args(name, &args, 2)?;
            let virtualized = Command::new("virt-what")
                .output()
                .is_ok_and(|output| output.status.success() && !output.stdout.is_empty());
            Ok(args[usize::from(!virtualized)].to_string())
        }
        _ => bail!("Unknown TuneD profile function '{name}'"),
    }
}

fn exact_args(name: &str, args: &[&str], expected: usize) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!(
            "TuneD function '{name}' requires {expected} argument(s), got {}",
            args.len()
        )
    }
}

fn min_args(name: &str, args: &[&str], minimum: usize) -> Result<()> {
    if args.len() >= minimum {
        Ok(())
    } else {
        bail!("TuneD function '{name}' requires at least {minimum} argument(s)")
    }
}

fn read(path: &str) -> Result<String> {
    fs::read_to_string(config::resolve_path(path)).with_context(|| format!("Failed to read {path}"))
}

fn execute(args: &[&str]) -> Result<String> {
    min_args("exec", args, 1)?;
    if args[0].is_empty() || args[0].contains('\0') {
        bail!("TuneD exec function requires an executable")
    }
    let output = Command::new(args[0]).args(&args[1..]).output()?;
    if !output.status.success() {
        bail!(
            "TuneD exec function '{}' failed with {}",
            args[0],
            output.status
        )
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn match_pairs(args: &[&str], haystack: &str) -> Result<String> {
    min_args("match function", args, 2)?;
    for pair in args.chunks(2) {
        if pair.len() == 2 && Regex::new(pair[0])?.is_match(haystack) {
            return Ok(pair[1].to_string());
        }
    }
    Ok(if args.len() % 2 == 1 {
        args.last().unwrap_or(&"")
    } else {
        ""
    }
    .to_string())
}

fn convert_units(
    name: &str,
    args: &[&str],
    convert: impl Fn(i64) -> Option<i64>,
) -> Result<String> {
    exact_args(name, args, 1)?;
    let value = args[0].parse::<i64>()?;
    Ok(convert(value)
        .ok_or_else(|| anyhow::anyhow!("TuneD function '{name}' overflow"))?
        .to_string())
}

fn parse_cpu_list(raw: &str) -> Result<Vec<u32>> {
    let mut included = BTreeSet::new();
    let mut excluded = BTreeSet::new();
    for group in raw.trim_matches(['\'', '"']).split(",,") {
        let group = group.trim();
        if group.is_empty() {
            continue;
        }
        if group.to_ascii_lowercase().starts_with("0x") {
            included.extend(hex_to_cpu_list(group)?);
            continue;
        }
        for field in group
            .split(',')
            .map(str::trim)
            .filter(|field| !field.is_empty())
        {
            let (negative, field) = match field.as_bytes().first() {
                Some(b'^' | b'!') => (true, &field[1..]),
                _ => (false, field),
            };
            let target = if negative {
                &mut excluded
            } else {
                &mut included
            };
            if let Some((start, end)) = field.split_once('-') {
                let start = start.parse::<u32>()?;
                let end = end.parse::<u32>()?;
                if start > end || end - start > 1_048_576 {
                    bail!("Invalid CPU range '{field}'");
                }
                target.extend(start..=end);
            } else {
                target.insert(field.parse::<u32>()?);
            }
        }
    }
    included.retain(|cpu| !excluded.contains(cpu));
    Ok(included.into_iter().collect())
}

fn pack_cpu_list(cpus: &[u32]) -> String {
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < cpus.len() {
        let start = cpus[index];
        let mut end = start;
        while index + 1 < cpus.len() && cpus[index + 1] == end + 1 {
            index += 1;
            end = cpus[index];
        }
        ranges.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        index += 1;
    }
    ranges.join(",")
}

fn join_cpus(cpus: &[u32]) -> String {
    cpus.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn invert_cpu_list(cpus: &[u32]) -> Result<Vec<u32>> {
    let excluded = cpus.iter().copied().collect::<BTreeSet<_>>();
    Ok(parse_cpu_list(&read("/sys/devices/system/cpu/online")?)?
        .into_iter()
        .filter(|cpu| !excluded.contains(cpu))
        .collect())
}

fn cpu_list_to_hex(cpus: &[u32]) -> Result<String> {
    let highest = cpus.last().copied().unwrap_or(0) as usize;
    if highest > 1_048_576 {
        bail!("CPU mask is too large");
    }
    let mut words = vec![0_u32; highest / 32 + 1];
    for cpu in cpus {
        words[*cpu as usize / 32] |= 1_u32 << (*cpu % 32);
    }
    Ok(words
        .into_iter()
        .rev()
        .map(|word| format!("{word:08x}"))
        .collect::<Vec<_>>()
        .join(","))
}

fn hex_to_cpu_list(raw: &str) -> Result<Vec<u32>> {
    let digits = raw
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .replace(',', "");
    if digits.is_empty() || digits.len() > 262_144 {
        bail!("Invalid hexadecimal CPU mask");
    }
    let mut cpus = Vec::new();
    for (nibble_index, digit) in digits.chars().rev().enumerate() {
        let nibble = digit
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("Invalid hexadecimal CPU mask"))?;
        for bit in 0..4 {
            if nibble & (1 << bit) != 0 {
                cpus.push((nibble_index * 4 + bit) as u32);
            }
        }
    }
    Ok(cpus)
}

fn calc_isolated_cores(args: &[&str]) -> Result<String> {
    if args.len() > 1 {
        bail!("calc_isolated_cores accepts at most one argument");
    }
    let reserve = args.first().copied().unwrap_or("1").parse::<usize>()?;
    let root = config::resolve_path("/sys/devices/system/cpu");
    let mut packages = BTreeMap::<String, Vec<u32>>::new();
    for entry in fs::read_dir(root)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name
            .strip_prefix("cpu")
            .and_then(|id| id.parse::<u32>().ok())
        else {
            continue;
        };
        let package = match fs::read_to_string(entry.path().join("topology/physical_package_id")) {
            Ok(package) => package.trim().to_string(),
            Err(_) => continue,
        };
        packages.entry(package).or_default().push(id);
    }
    let mut isolated = Vec::new();
    for cpus in packages.values_mut() {
        cpus.sort_unstable();
        isolated.extend(cpus.iter().skip(reserve).copied());
    }
    isolated.sort_unstable();
    Ok(pack_cpu_list(&isolated))
}

fn package_devices(args: &[&str], uncore: bool) -> Result<String> {
    min_args(
        if uncore {
            "package2uncores"
        } else {
            "package2cpus"
        },
        args,
        1,
    )?;
    let root = config::resolve_path(if uncore {
        "/sys/devices/system/cpu/intel_uncore_frequency"
    } else {
        "/sys/devices/system/cpu"
    });
    let mut devices = Vec::new();
    for entry in fs::read_dir(root)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let package = if uncore {
            if name.starts_with("uncore") {
                fs::read_to_string(entry.path().join("package_id")).ok()
            } else if name.starts_with("package_") {
                name.get(8..10).map(str::to_string)
            } else {
                None
            }
        } else if name
            .strip_prefix("cpu")
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        {
            fs::read_to_string(entry.path().join("topology/physical_package_id")).ok()
        } else {
            None
        };
        let Some(package) = package else { continue };
        let package = package.trim();
        if args.iter().any(|pattern| wildcard_match(pattern, package)) {
            devices.push(name);
        }
    }
    devices.sort();
    if devices.is_empty() {
        bail!("No devices match the requested CPU package")
    }
    Ok(devices.join(","))
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = format!(
        "^{}$",
        regex::escape(pattern)
            .replace(r"\*", ".*")
            .replace(r"\?", ".")
    );
    Regex::new(&pattern).is_ok_and(|regex| regex.is_match(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_list_function_family_matches_tuned_encodings() {
        assert_eq!(evaluate("cpulist_unpack:1-3,5").unwrap(), "1,2,3,5");
        assert_eq!(evaluate("cpulist_pack:1,2,3,5").unwrap(), "1-3,5");
        assert_eq!(evaluate("cpulist2hex:0-3").unwrap(), "0000000f");
        assert_eq!(evaluate("hex2cpulist:0000000a").unwrap(), "1,3");
        assert_eq!(evaluate("cpulist2devs:1-2").unwrap(), "cpu1,cpu2");
    }

    #[test]
    fn assertions_regex_units_and_stripping_are_fail_closed() {
        assert_eq!(evaluate("assertion:same:x:x").unwrap(), "");
        assert!(evaluate("assertion:different:x:y").is_err());
        assert_eq!(
            evaluate("regex_search_ternary:yes:^y:on:off").unwrap(),
            "on"
        );
        assert_eq!(evaluate("kb2s:32").unwrap(), "64");
        assert_eq!(evaluate("s2kb:65").unwrap(), "32");
        assert_eq!(evaluate("s2kb:67").unwrap(), "34");
        assert_eq!(evaluate("s2kb:-67").unwrap(), "-34");
        assert_eq!(evaluate("strip:  a :b  ").unwrap(), "a b");
        assert!(evaluate("missing:value").is_err());
    }

    #[test]
    fn assertions_preserve_colons_inside_the_compared_value() {
        assert_eq!(
            evaluate("assertion_non_equal:value is set:cstate.name:C1|10:${value}").unwrap(),
            ""
        );
    }
}
