use std::path::{Path, PathBuf};

pub const NAMESPACE: &str = "com.redhat.tuned";
pub const DBUS_INTERFACE: &str = "com.redhat.tuned.control";
pub const DBUS_OBJECT: &str = "/Tuned";

pub const GLOBAL_CONFIG_FILE: &str = "/etc/tuned/tuned-main.conf";
pub const ACTIVE_PROFILE_FILE: &str = "/etc/tuned/active_profile";
pub const PROFILE_MODE_FILE: &str = "/etc/tuned/profile_mode";
pub const PROFILE_FILE: &str = "tuned.conf";
pub const DEFAULT_PROFILE: &str = "balanced";
pub const ROLLBACK_FILE: &str = "/var/lib/tuned-rs/rollback.json";

pub const SYSTEM_PROFILES_DIR: &str = "/usr/lib/tuned/profiles";
pub const USER_PROFILES_DIR: &str = "/etc/tuned/profiles";

pub const ROLLBACK_AUTO: &str = "auto";
pub const ROLLBACK_NOT_ON_EXIT: &str = "not_on_exit";

pub fn default_profile_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from(SYSTEM_PROFILES_DIR),
        PathBuf::from(USER_PROFILES_DIR),
    ]
}

pub fn profile_dirs_from_env() -> Vec<PathBuf> {
    std::env::var("TUNED_RS_PROFILE_DIRS")
        .ok()
        .map(|value| {
            value
                .split([',', ';'])
                .filter(|part| !part.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .filter(|dirs: &Vec<PathBuf>| !dirs.is_empty())
        .unwrap_or_else(default_profile_dirs)
}

pub fn resolve_path(base: &str) -> PathBuf {
    if let Ok(root) = std::env::var("TUNED_RS_ROOT") {
        PathBuf::from(root).join(base.trim_start_matches('/'))
    } else {
        PathBuf::from(base)
    }
}

pub fn resolve_path_buf(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if let Ok(root) = std::env::var("TUNED_RS_ROOT") {
        if path.is_absolute() {
            PathBuf::from(root).join(path.strip_prefix("/").unwrap_or(path))
        } else {
            PathBuf::from(root).join(path)
        }
    } else {
        path.to_path_buf()
    }
}

pub fn rollback_on_exit() -> bool {
    let path = resolve_path(GLOBAL_CONFIG_FILE);
    if !path.is_file() {
        return true;
    }

    let mut ini = configparser::ini::Ini::new();
    if ini.load(path.to_str().unwrap_or_default()).is_err() {
        return true;
    }

    !matches!(
        ini.get("main", "rollback").as_deref(),
        Some(ROLLBACK_NOT_ON_EXIT)
    )
}
