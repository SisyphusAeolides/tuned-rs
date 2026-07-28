use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde_json::json;

#[derive(Debug, Clone, Default)]
pub struct SignalRegistry {
    paths: BTreeSet<PathBuf>,
}

impl SignalRegistry {
    pub fn from_paths(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut registry = Self::default();
        for path in paths {
            registry.register(path);
        }
        registry
    }

    pub fn register(&mut self, path: impl AsRef<Path>) -> bool {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return false;
        }
        self.paths.insert(path.to_path_buf());
        true
    }

    pub fn emit_profile_changed(
        &self,
        profile_name: &str,
        result: bool,
        error: &str,
    ) -> Vec<(PathBuf, String)> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "profile_changed",
            "params": [profile_name, result, error],
        })
        .to_string();

        let mut failures = Vec::new();
        for path in &self.paths {
            match UnixStream::connect(path) {
                Ok(mut socket) => {
                    if let Err(error) = socket.write_all(payload.as_bytes()) {
                        failures.push((path.clone(), error.to_string()));
                    }
                }
                Err(error) => failures.push((path.clone(), error.to_string())),
            }
        }
        failures
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.paths.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::net::UnixListener;
    use std::thread;
    use tempfile::TempDir;

    #[test]
    fn registration_is_idempotent() {
        let mut registry = SignalRegistry::default();
        assert!(registry.register("/run/tuned/one.sock"));
        assert!(registry.register("/run/tuned/one.sock"));
        assert_eq!(registry.len(), 1);
        assert!(!registry.register(""));
    }

    #[test]
    fn profile_change_uses_upstream_json_rpc_envelope() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("signal.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let receiver = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut payload = String::new();
            stream.read_to_string(&mut payload).unwrap();
            payload
        });

        let mut registry = SignalRegistry::default();
        assert!(registry.register(&path));
        assert!(registry
            .emit_profile_changed("balanced", true, "")
            .is_empty());
        let payload = receiver.join().unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["method"], "profile_changed");
        assert_eq!(value["params"], json!(["balanced", true, ""]));
    }
}
