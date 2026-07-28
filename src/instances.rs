use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::{engine, plugins};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicInstance {
    pub name: String,
    pub plugin: String,
    pub priority: i32,
    pub primary: bool,
    pub options: HashMap<String, String>,
    pub devices: BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub struct InstanceRegistry {
    instances: BTreeMap<String, DynamicInstance>,
    owners: BTreeMap<String, String>,
}

impl InstanceRegistry {
    pub fn create(
        &mut self,
        plugin_name: &str,
        instance_name: &str,
        options: HashMap<String, String>,
    ) -> (bool, String) {
        if !valid_instance_name(instance_name) {
            return (false, "Invalid instance name".to_string());
        }
        if plugins::descriptor(plugin_name).is_none() {
            return (false, format!("Plugin '{plugin_name}' not found"));
        }
        if !supports_dynamic_instances(plugin_name) {
            return (
                false,
                format!(
                    "Plugin '{plugin_name}' does not support hotplugging or dynamic instances."
                ),
            );
        }
        if self.instances.contains_key(instance_name) {
            return (false, format!("Instance '{instance_name}' already exists"));
        }

        let devices =
            match parse_device_list(options.get("devices").map(String::as_str).unwrap_or("")) {
                Ok(devices) => devices,
                Err(error) => return (false, error),
            };
        let priority = match options.get("priority") {
            Some(value) => match value.parse::<i32>() {
                Ok(priority) => priority,
                Err(_) => return (false, "Invalid priority".to_string()),
            },
            None => 0,
        };

        let transfers = match self.plan_transfers(plugin_name, &devices, false) {
            Ok(transfers) => transfers,
            Err(error) => return (false, error),
        };
        self.commit_transfers(instance_name, &transfers);

        let primary = !self
            .instances
            .values()
            .any(|instance| instance.plugin == plugin_name);
        let instance = DynamicInstance {
            name: instance_name.to_string(),
            plugin: plugin_name.to_string(),
            priority,
            primary,
            options,
            devices: devices.clone(),
        };
        for device in devices {
            self.owners.insert(device, instance_name.to_string());
        }
        self.instances.insert(instance_name.to_string(), instance);
        debug_assert!(self.invariant_holds());
        (true, "OK".to_string())
    }

    pub fn acquire_devices(&mut self, devices: &str, instance_name: &str) -> (bool, String) {
        let Some(target) = self.instances.get(instance_name) else {
            return (false, format!("Instance '{instance_name}' not found"));
        };
        let plugin_name = target.plugin.clone();
        let requested = match parse_device_list(devices) {
            Ok(devices) => devices,
            Err(error) => return (false, error),
        };

        let transfers = match self.plan_transfers(&plugin_name, &requested, true) {
            Ok(transfers) => transfers,
            Err(error) => return (false, error),
        };
        self.commit_transfers(instance_name, &transfers);

        if let Some(target) = self.instances.get_mut(instance_name) {
            for device in requested {
                target.devices.insert(device.clone());
                self.owners.insert(device, instance_name.to_string());
            }
        }
        debug_assert!(self.invariant_holds());
        (true, "OK".to_string())
    }

    pub fn destroy(&mut self, instance_name: &str) -> (bool, String) {
        let Some(instance) = self.instances.remove(instance_name) else {
            return (false, format!("Instance '{instance_name}' not found"));
        };
        for device in instance.devices {
            if self
                .owners
                .get(&device)
                .is_some_and(|owner| owner == instance_name)
            {
                self.owners.remove(&device);
            }
        }
        debug_assert!(self.invariant_holds());
        (true, "OK".to_string())
    }

    pub fn list(&self, plugin_name: &str) -> Vec<(String, String)> {
        self.instances
            .values()
            .filter(|instance| plugin_name.is_empty() || instance.plugin == plugin_name)
            .map(|instance| (instance.name.clone(), instance.plugin.clone()))
            .collect()
    }

    pub fn devices(&self, instance_name: &str) -> Option<Vec<String>> {
        self.instances
            .get(instance_name)
            .map(|instance| instance.devices.iter().cloned().collect())
    }

    pub fn instance(&self, instance_name: &str) -> Option<DynamicInstance> {
        self.instances.get(instance_name).cloned()
    }

    pub fn all_instances(&self) -> Vec<DynamicInstance> {
        self.instances.values().cloned().collect()
    }

    fn plan_transfers(
        &self,
        plugin_name: &str,
        devices: &BTreeSet<String>,
        require_existing_owner: bool,
    ) -> Result<Vec<(String, String)>, String> {
        let mut transfers = Vec::new();
        let mut unhandled = BTreeSet::new();

        for device in devices {
            let Some(owner_name) = self.owners.get(device) else {
                if require_existing_owner {
                    unhandled.insert(device.clone());
                }
                continue;
            };
            let Some(owner) = self.instances.get(owner_name) else {
                return Err(format!("Ownership index for '{device}' is inconsistent"));
            };
            if owner.plugin != plugin_name {
                return Err(format!(
                    "Target instance is of type '{plugin_name}', but device '{device}' is handled by instance '{}' of type '{}'.",
                    owner.name, owner.plugin
                ));
            }
            transfers.push((device.clone(), owner_name.clone()));
        }

        if !unhandled.is_empty() {
            return Err(format!(
                "Ignoring devices not handled by any instance '{}'.",
                format_device_set(&unhandled)
            ));
        }
        Ok(transfers)
    }

    fn commit_transfers(&mut self, target: &str, transfers: &[(String, String)]) {
        for (device, previous_owner) in transfers {
            if previous_owner == target {
                continue;
            }
            if let Some(instance) = self.instances.get_mut(previous_owner) {
                instance.devices.remove(device);
            }
        }
    }

    fn invariant_holds(&self) -> bool {
        self.owners.iter().all(|(device, owner_name)| {
            self.instances
                .get(owner_name)
                .is_some_and(|instance| instance.devices.contains(device))
        }) && self.instances.iter().all(|(instance_name, instance)| {
            instance
                .devices
                .iter()
                .all(|device| self.owners.get(device) == Some(instance_name))
        })
    }
}

pub fn supports_dynamic_instances(plugin_name: &str) -> bool {
    matches!(
        plugin_name,
        "audio" | "cpu" | "disk" | "irq" | "net" | "network" | "scsi_host" | "uncore"
    )
}

fn valid_instance_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn parse_device_list(raw: &str) -> Result<BTreeSet<String>, String> {
    let mut devices = BTreeSet::new();
    let mut current = String::new();
    let mut escaped = false;

    for character in raw.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == ',' {
            insert_device(&mut devices, &current)?;
            current.clear();
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    insert_device(&mut devices, &current)?;
    Ok(devices)
}

fn insert_device(devices: &mut BTreeSet<String>, raw: &str) -> Result<(), String> {
    let device = raw.trim();
    if device.is_empty() {
        return Ok(());
    }
    if !engine::validate_tuned_argument(device) {
        return Err("Invalid devices".to_string());
    }
    devices.insert(device.to_string());
    Ok(())
}

fn format_device_set(devices: &BTreeSet<String>) -> String {
    devices.iter().cloned().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(devices: &str) -> HashMap<String, String> {
        HashMap::from([
            ("devices".to_string(), devices.to_string()),
            ("priority".to_string(), "10".to_string()),
        ])
    }

    #[test]
    fn create_and_list_instances() {
        let mut registry = InstanceRegistry::default();
        assert_eq!(
            registry.create("disk", "latency", options("sda,sdb")),
            (true, "OK".to_string())
        );
        assert_eq!(
            registry.list("disk"),
            vec![("latency".to_string(), "disk".to_string())]
        );
        assert_eq!(
            registry.devices("latency"),
            Some(vec!["sda".to_string(), "sdb".to_string()])
        );
        assert!(registry.invariant_holds());
    }

    #[test]
    fn only_the_first_instance_of_a_plugin_is_primary() {
        let mut registry = InstanceRegistry::default();
        assert!(registry.create("cpu", "first", options("cpu0")).0);
        assert!(registry.create("cpu", "second", options("cpu1")).0);
        assert!(registry.instance("first").unwrap().primary);
        assert!(!registry.instance("second").unwrap().primary);
    }

    #[test]
    fn same_plugin_transfer_is_exclusive() {
        let mut registry = InstanceRegistry::default();
        assert!(registry.create("disk", "first", options("sda,sdb")).0);
        assert!(registry.create("disk", "second", options("sdc")).0);
        assert_eq!(
            registry.acquire_devices("sdb,sdc", "second"),
            (true, "OK".to_string())
        );
        assert_eq!(registry.devices("first"), Some(vec!["sda".to_string()]));
        assert_eq!(
            registry.devices("second"),
            Some(vec!["sdb".to_string(), "sdc".to_string()])
        );
        assert!(registry.invariant_holds());
    }

    #[test]
    fn failed_acquire_is_transactional() {
        let mut registry = InstanceRegistry::default();
        assert!(registry.create("disk", "first", options("sda")).0);
        assert!(registry.create("disk", "second", options("sdb")).0);
        let before = registry.devices("second");
        let result = registry.acquire_devices("sda,sdz", "second");
        assert!(!result.0);
        assert_eq!(registry.devices("second"), before);
        assert_eq!(registry.devices("first"), Some(vec!["sda".to_string()]));
        assert!(registry.invariant_holds());
    }

    #[test]
    fn cross_plugin_transfer_is_rejected() {
        let mut registry = InstanceRegistry::default();
        assert!(registry.create("irq", "processor", options("irq1")).0);
        assert!(registry.create("disk", "storage", options("sda")).0);
        let result = registry.acquire_devices("irq1", "storage");
        assert!(!result.0);
        assert_eq!(
            registry.devices("processor"),
            Some(vec!["irq1".to_string()])
        );
        assert!(registry.invariant_holds());
    }

    #[test]
    fn destroy_releases_every_device() {
        let mut registry = InstanceRegistry::default();
        assert!(registry.create("disk", "temporary", options("sda,sdb")).0);
        assert_eq!(registry.destroy("temporary"), (true, "OK".to_string()));
        assert!(registry.list("").is_empty());
        assert!(registry.owners.is_empty());
        assert!(registry.invariant_holds());
    }

    #[test]
    fn rejects_instance_names_that_can_escape_the_journal_directory() {
        let mut registry = InstanceRegistry::default();
        assert!(!registry.create("disk", "../escape", options("sda")).0);
        assert!(!registry.create("disk", "", options("sda")).0);
        assert!(registry.create("disk", "safe.instance-1", options("sda")).0);
    }

    #[test]
    fn escaped_commas_are_preserved_in_device_names() {
        assert_eq!(
            parse_device_list(r"device\,with-comma,device1").unwrap(),
            BTreeSet::from(["device,with-comma".to_string(), "device1".to_string()])
        );
    }
}
