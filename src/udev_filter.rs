use std::collections::BTreeSet;

use anyhow::Result;
use regex::Regex;
use tokio_udev::Enumerator;

pub fn matching_names(subsystem: &str, pattern: Option<&str>) -> Result<Vec<String>> {
    let regex = pattern.map(Regex::new).transpose()?;
    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem(subsystem)?;
    let mut names = BTreeSet::new();
    for device in enumerator.scan_devices()? {
        if regex.as_ref().is_some_and(|regex| {
            let mut properties = device
                .properties()
                .filter_map(|property| {
                    Some((
                        property.name().to_str()?.to_string(),
                        property.value().to_str()?.to_string(),
                    ))
                })
                .collect::<Vec<_>>();
            properties.sort_unstable();
            let text = properties
                .into_iter()
                .map(|(name, value)| format!("{name}={value}\n"))
                .collect::<String>();
            !regex.is_match(&text)
        }) {
            continue;
        }
        if let Some(name) = device.sysname().to_str() {
            names.insert(name.to_string());
        }
    }
    Ok(names.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_udev_regex_fails_closed() {
        assert!(matching_names("net", Some("[")).is_err());
    }
}
