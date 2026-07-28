use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use tuned_rs::plugins;
use tuned_rs::profile::ProfileCatalog;
use tuned_rs::profile_runtime;

#[test]
fn upstream_profile_surface_is_supported() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let Some(root) = std::env::var_os("TUNED_RS_UPSTREAM_PROFILES") else {
        return;
    };
    let root = PathBuf::from(root);
    let expected = profile_names(&root);
    let catalog = ProfileCatalog::load_from_dirs(std::slice::from_ref(&root)).unwrap();
    let loaded = catalog.names().into_iter().collect::<BTreeSet<_>>();
    let missing = expected.difference(&loaded).cloned().collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "profiles skipped during parsing: {missing:?}"
    );

    let mut gaps = BTreeSet::new();
    for name in loaded {
        let profile = catalog.get(&name).unwrap();
        let units = profile_runtime::active_units(profile).unwrap_or_else(|error| {
            if !error
                .to_string()
                .contains("Failed to read variables file /etc/tuned/")
            {
                gaps.insert(format!("{name}: runtime expansion failed: {error}"));
            }
            profile.units.clone()
        });
        for unit in units {
            let Some(descriptor) = plugins::descriptor(&unit.plugin_type) else {
                gaps.insert(format!("{name}: missing plugin {}", unit.plugin_type));
                continue;
            };
            for (option, _) in unit.options {
                if dynamic_option(&unit.plugin_type, &option)
                    || descriptor
                        .options
                        .iter()
                        .any(|supported| supported.name == option)
                {
                    continue;
                }
                gaps.insert(format!(
                    "{name}: unsupported option {}.{option}",
                    unit.plugin_type
                ));
            }
        }
    }
    assert!(
        gaps.is_empty(),
        "upstream profile compatibility gaps:\n{}",
        gaps.into_iter().collect::<Vec<_>>().join("\n")
    );
}

fn profile_names(root: &Path) -> BTreeSet<String> {
    fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().join("tuned.conf").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

fn dynamic_option(plugin: &str, option: &str) -> bool {
    match plugin {
        "modules" | "sysctl" | "sysfs" => true,
        "bootloader" => option.starts_with("cmdline"),
        "scheduler" => option.starts_with("group.") || option.starts_with("cgroup."),
        "service" => option.starts_with("service."),
        _ => false,
    }
}
