use std::collections::HashMap;
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;
use tracing::warn;

use crate::daemon::Daemon;
use crate::{config, log_capture, plugins};

pub struct Server {
    path: PathBuf,
    worker: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.worker.abort();
        if fs::symlink_metadata(&self.path).is_ok_and(|meta| meta.file_type().is_socket()) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn spawn(daemon: Arc<Daemon>) -> Result<Option<Server>> {
    if !config::unix_socket_enabled() {
        return Ok(None);
    }
    let path = config::unix_socket_path();
    validate_socket_path(&path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if !metadata.file_type().is_socket() {
            bail!("Refusing to replace non-socket path {}", path.display());
        }
        fs::remove_file(&path)?;
    }
    let listener = std::os::unix::net::UnixListener::bind(&path)
        .with_context(|| format!("Failed to bind TuneD socket {}", path.display()))?;
    listener.set_nonblocking(true)?;
    let backlog = i32::try_from(config::unix_socket_backlog()).unwrap_or(i32::MAX);
    // SAFETY: the listener owns a valid AF_UNIX socket descriptor and `listen`
    // only updates its already-established accept queue length.
    if unsafe { libc::listen(listener.as_raw_fd(), backlog) } != 0 {
        return Err(std::io::Error::last_os_error()).context("Failed to set Unix-socket backlog");
    }
    let listener = UnixListener::from_std(listener)?;
    fs::set_permissions(
        &path,
        fs::Permissions::from_mode(config::unix_socket_permissions()),
    )?;
    apply_ownership(&path, config::unix_socket_ownership())?;
    let worker = tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(connection) => connection,
                Err(error) => {
                    warn!("TuneD Unix-socket accept failed: {error}");
                    break;
                }
            };
            let daemon = Arc::clone(&daemon);
            tokio::spawn(async move {
                if let Err(error) = serve_connection(stream, daemon).await {
                    warn!("TuneD Unix-socket request failed: {error}");
                }
            });
        }
    });
    Ok(Some(Server { path, worker }))
}

fn validate_socket_path(path: &std::path::Path) -> Result<()> {
    let logical = if std::env::var_os("TUNED_RS_ROOT").is_some() {
        path.strip_prefix(config::resolve_path("/"))
            .map_err(|_| anyhow::anyhow!("TuneD Unix socket escapes the configured root"))?
    } else {
        path.strip_prefix("/")
            .map_err(|_| anyhow::anyhow!("TuneD Unix socket path must be absolute"))?
    };
    if logical.components().count() < 2
        || !logical
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        bail!("Invalid TuneD Unix-socket path {}", path.display());
    }
    Ok(())
}

fn apply_ownership(path: &std::path::Path, ownership: (Option<u32>, Option<u32>)) -> Result<()> {
    let (uid, gid) = ownership;
    if uid.is_none() && gid.is_none() {
        return Ok(());
    }
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: `path` is a NUL-terminated path owned for the duration of the
    // call; `u32::MAX` is the POSIX sentinel for an unchanged owner/group.
    if unsafe {
        libc::chown(
            path.as_ptr(),
            uid.unwrap_or(u32::MAX),
            gid.unwrap_or(u32::MAX),
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error()).context("Failed to set Unix-socket ownership");
    }
    Ok(())
}

async fn serve_connection(mut stream: UnixStream, daemon: Arc<Daemon>) -> Result<()> {
    let mut input = Vec::new();
    stream.read_to_end(&mut input).await?;
    let request: Value = match serde_json::from_slice(&input) {
        Ok(request) => request,
        Err(error) => {
            write_response(
                &mut stream,
                &rpc_error(Value::Null, -32700, "Parse error", Some(error.to_string())),
            )
            .await?;
            return Ok(());
        }
    };
    let response = if let Some(batch) = request.as_array() {
        if batch.is_empty() {
            Some(rpc_error(Value::Null, -32600, "Invalid Request", None))
        } else {
            let mut responses = Vec::new();
            for request in batch {
                if let Some(response) = process_request(request, &daemon).await {
                    responses.push(response);
                }
            }
            (!responses.is_empty()).then_some(Value::Array(responses))
        }
    } else {
        process_request(&request, &daemon).await
    };
    if let Some(response) = response {
        write_response(&mut stream, &response).await?;
    }
    Ok(())
}

async fn write_response(stream: &mut UnixStream, response: &Value) -> Result<()> {
    stream.write_all(&serde_json::to_vec(response)?).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn process_request(request: &Value, daemon: &Arc<Daemon>) -> Option<Value> {
    let Some(object) = request.as_object() else {
        return Some(rpc_error(Value::Null, -32600, "Invalid Request", None));
    };
    let id = object.get("id").cloned();
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object.get("method").and_then(Value::as_str).is_none()
    {
        return Some(rpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "Invalid Request",
            None,
        ));
    }
    let method = object["method"].as_str().unwrap();
    let params = object.get("params").unwrap_or(&Value::Null);
    let result = dispatch(method, params, daemon).await;
    let id = id?;
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(DispatchError::Method) => rpc_error(id, -32601, "Method not found", None),
        Err(DispatchError::Params(detail)) => rpc_error(id, -32602, "Invalid params", Some(detail)),
        Err(DispatchError::Other(detail)) => rpc_error(id, 1, "Error", Some(detail)),
    })
}

#[derive(Debug)]
enum DispatchError {
    Method,
    Params(String),
    Other(String),
}

async fn dispatch(
    method: &str,
    params: &Value,
    daemon: &Arc<Daemon>,
) -> std::result::Result<Value, DispatchError> {
    let p = |index, name| parameter(params, index, name);
    let result = match method {
        "start" => json!(daemon.start().await.map_err(other)?),
        "stop" => json!(daemon.stop(true).await),
        "reload" => {
            let stopped = daemon.stop(false).await;
            let loaded = daemon.reload_catalog().await.map_err(other).is_ok();
            json!(stopped && loaded && daemon.start().await.map_err(other)?)
        }
        "switch_profile" => json!(
            daemon
                .switch_profile(string(p(0, "profile_name")?)?, true)
                .await
        ),
        "auto_profile" => {
            let profile = daemon.recommend_profile().await;
            json!(daemon.switch_profile(&profile, false).await)
        }
        "active_profile" => json!(daemon.active_profile().await),
        "profile_mode" => json!(daemon.profile_mode().await),
        "post_loaded_profile" => json!(daemon.post_loaded_profile().await),
        "disable" => json!(daemon.disable().await),
        "is_running" => json!(daemon.is_running().await),
        "profiles" => json!(daemon.profiles().await),
        "profiles2" => json!(daemon.profiles2().await),
        "profile_info" => json!(daemon.profile_info(string(p(0, "profile_name")?)?).await),
        "recommend_profile" => json!(daemon.recommend_profile().await),
        "verify_profile" => json!(daemon.verify_active_profile(false).await),
        "verify_profile_ignore_missing" => json!(daemon.verify_active_profile(true).await),
        "log_capture_start" => {
            let level = integer(p(0, "log_level")?)?;
            let timeout = integer(p(1, "timeout")?)?;
            json!(log_capture::global_store().start(level, timeout))
        }
        "log_capture_finish" => json!(log_capture::global_store().finish(string(p(0, "token")?)?)),
        "get_all_plugins" => json!(plugins::all_options()),
        "get_plugin_documentation" => json!(plugins::documentation(string(p(0, "plugin_name")?)?)),
        "get_plugin_hints" => json!(plugins::hints(string(p(0, "plugin_name")?)?)),
        "register_socket_signal_path" => json!(
            daemon
                .register_socket_signal_path(string(p(0, "path")?)?)
                .await
        ),
        "instance_acquire_devices" => json!(
            daemon
                .instance_acquire_devices(
                    string(p(0, "devices")?)?,
                    string(p(1, "instance_name")?)?
                )
                .await
        ),
        "get_instances" => {
            let plugin = string(p(0, "plugin_name")?)?;
            json!((true, "OK", daemon.get_instances(plugin).await))
        }
        "instance_get_devices" => {
            let name = string(p(0, "instance_name")?)?;
            match daemon.instance_get_devices(name).await {
                Some(devices) => json!((true, "OK", devices)),
                None => json!((
                    false,
                    format!("Instance '{name}' not found"),
                    Vec::<String>::new()
                )),
            }
        }
        "instance_create" => {
            let plugin = string(p(0, "plugin_name")?)?;
            let instance = string(p(1, "instance_name")?)?;
            let options = string_map(p(2, "options")?)?;
            json!(daemon.instance_create(plugin, instance, options).await)
        }
        "instance_destroy" => json!(
            daemon
                .instance_destroy(string(p(0, "instance_name")?)?)
                .await
        ),
        _ => return Err(DispatchError::Method),
    };
    Ok(result)
}

fn parameter<'a>(
    params: &'a Value,
    index: usize,
    name: &str,
) -> std::result::Result<&'a Value, DispatchError> {
    if let Some(values) = params.as_array() {
        values
            .get(index)
            .ok_or_else(|| DispatchError::Params(format!("missing parameter '{name}'")))
    } else if let Some(values) = params.as_object() {
        values
            .get(name)
            .ok_or_else(|| DispatchError::Params(format!("missing parameter '{name}'")))
    } else {
        Err(DispatchError::Params(format!("missing parameter '{name}'")))
    }
}

fn string(value: &Value) -> std::result::Result<&str, DispatchError> {
    value
        .as_str()
        .ok_or_else(|| DispatchError::Params("expected string".to_string()))
}

fn integer(value: &Value) -> std::result::Result<i32, DispatchError> {
    value
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| DispatchError::Params("expected 32-bit integer".to_string()))
}

fn string_map(value: &Value) -> std::result::Result<HashMap<String, String>, DispatchError> {
    serde_json::from_value(value.clone()).map_err(|error| DispatchError::Params(error.to_string()))
}

fn other(error: impl std::fmt::Display) -> DispatchError {
    DispatchError::Other(error.to_string())
}

fn rpc_error(id: Value, code: i32, message: &str, data: Option<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message, "data": data}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ProfileCatalog;
    use crate::rollback::Rollback;
    use tempfile::TempDir;

    #[test]
    fn accepts_positional_and_named_parameters() {
        assert_eq!(
            parameter(&json!(["balanced"]), 0, "profile_name").unwrap(),
            "balanced"
        );
        assert_eq!(
            parameter(&json!({"profile_name": "balanced"}), 0, "profile_name").unwrap(),
            "balanced"
        );
        assert!(parameter(&Value::Null, 0, "profile_name").is_err());
    }

    #[test]
    fn socket_path_must_be_absolute_normalized_and_non_broad() {
        let _guard = config::test_env_lock();
        std::env::remove_var("TUNED_RS_ROOT");
        assert!(validate_socket_path(&config::resolve_path("/run/tuned/tuned.sock")).is_ok());
        assert!(validate_socket_path(std::path::Path::new("relative.sock")).is_err());
        assert!(validate_socket_path(std::path::Path::new("/tuned.sock")).is_err());
        assert!(validate_socket_path(std::path::Path::new("/run/../etc/tuned.sock")).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn serves_upstream_json_rpc_requests_and_batches() {
        let _guard = config::test_env_lock();
        let root = TempDir::new().unwrap();
        let config_dir = root.path().join("etc/tuned");
        let profiles = root.path().join("profiles");
        fs::create_dir_all(&config_dir).unwrap();
        fs::create_dir_all(&profiles).unwrap();
        fs::write(
            config_dir.join("tuned-main.conf"),
            "[main]\nenable_unix_socket=true\nunix_socket_path=/run/tuned/test.sock\n",
        )
        .unwrap();
        std::env::set_var("TUNED_RS_ROOT", root.path());
        let catalog = ProfileCatalog::load_from_dirs(&[profiles]).unwrap();
        let daemon = Daemon::new(catalog, Arc::new(Rollback::load().unwrap()));
        let server = spawn(daemon).unwrap().unwrap();
        let mut client = UnixStream::connect(config::resolve_path("/run/tuned/test.sock"))
            .await
            .unwrap();
        client
            .write_all(br#"[{"jsonrpc":"2.0","method":"profiles","id":1},{"jsonrpc":"2.0","method":"active_profile","id":2}]"#)
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        let response: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response.as_array().unwrap().len(), 2);
        drop(server);
        std::env::remove_var("TUNED_RS_ROOT");
    }
}
