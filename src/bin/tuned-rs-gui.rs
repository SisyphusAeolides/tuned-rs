use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Read as _;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use zbus::proxy;

const MAX_REQUEST: usize = 64 * 1024;
const IDLE_TIMEOUT_SECONDS: u64 = 20;
const HTML: &str = include_str!("../../assets/gui/index.html");
const ICON: &str = include_str!("../../assets/icons/tuned-circle-gauge.svg");
static TELEMETRY: OnceLock<Mutex<tuned_rs::telemetry::TelemetryCollector>> = OnceLock::new();

#[proxy(
    interface = "com.redhat.tuned.control",
    default_service = "com.redhat.tuned",
    default_path = "/Tuned"
)]
trait Tuned {
    #[zbus(name = "active_profile")]
    fn active_profile(&self) -> zbus::Result<String>;
    #[zbus(name = "profiles2")]
    fn profiles2(&self) -> zbus::Result<Vec<(String, String)>>;
    #[zbus(name = "switch_profile")]
    fn switch_profile(&self, profile_name: &str) -> zbus::Result<(bool, String)>;
    #[zbus(name = "instance_create")]
    fn instance_create(
        &self,
        plugin_name: &str,
        instance_name: &str,
        options: HashMap<String, String>,
    ) -> zbus::Result<(bool, String)>;
    #[zbus(name = "instance_destroy")]
    fn instance_destroy(&self, instance_name: &str) -> zbus::Result<(bool, String)>;
}

#[derive(Deserialize)]
struct ProfileRequest {
    profile: String,
}

#[derive(Deserialize)]
struct CpuRequest {
    devices: String,
    governor: String,
    energy_performance_preference: String,
}

#[derive(Deserialize)]
struct NetworkRequest {
    devices: String,
    mtu: u32,
    wake_on_lan: String,
}

struct HttpRequest {
    method: String,
    path: String,
    token: Option<String>,
    body: Vec<u8>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let token = session_token()?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let address = listener.local_addr()?;
    let url = format!("http://{address}/#token={token}");
    open_browser(&url)?;
    eprintln!("TuneD Control Center is available at {url}");
    let last_activity = Arc::new(AtomicU64::new(unix_time()));

    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => Some(accepted?),
            () = tokio::time::sleep(std::time::Duration::from_secs(5)) => None,
        };
        let Some((stream, peer)) = accepted else {
            if unix_time().saturating_sub(last_activity.load(Ordering::Relaxed))
                >= IDLE_TIMEOUT_SECONDS
            {
                break;
            }
            continue;
        };
        if !peer.ip().is_loopback() {
            continue;
        }
        let token = token.clone();
        let last_activity = Arc::clone(&last_activity);
        tokio::spawn(async move {
            if let Err(error) = serve(stream, &token, &last_activity).await {
                eprintln!("GUI request failed: {error:#}");
            }
        });
    }
    Ok(())
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn session_token() -> Result<String> {
    let mut bytes = [0_u8; 24];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().fold(String::new(), |mut token, byte| {
        let _ = write!(token, "{byte:02x}");
        token
    }))
}

fn open_browser(url: &str) -> Result<()> {
    let status = Command::new("xdg-open")
        .arg(url)
        .spawn()
        .context("Failed to launch the desktop browser")?;
    drop(status);
    Ok(())
}

async fn serve(mut stream: TcpStream, token: &str, last_activity: &AtomicU64) -> Result<()> {
    let request = read_request(&mut stream).await?;
    if matches!(request.path.as_str(), "/" | "/icon.svg") || request.token.as_deref() == Some(token)
    {
        last_activity.store(unix_time(), Ordering::Relaxed);
    }
    let response = route(request, token).await;
    let (status, content_type, body) = match response {
        Ok((content_type, body)) => ("200 OK", content_type, body),
        Err(error) => (
            "400 Bad Request",
            "application/json; charset=utf-8",
            json!({"ok": false, "message": error.to_string()})
                .to_string()
                .into_bytes(),
        ),
    };
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self'\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn route(request: HttpRequest, token: &str) -> Result<(&'static str, Vec<u8>)> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => Ok(("text/html; charset=utf-8", HTML.as_bytes().to_vec())),
        ("GET", "/icon.svg") => Ok(("image/svg+xml", ICON.as_bytes().to_vec())),
        _ if request.token.as_deref() != Some(token) => bail!("Invalid GUI session token"),
        ("GET", "/api/state") => {
            let connection = zbus::Connection::system().await?;
            let proxy = TunedProxy::new(&connection).await?;
            let profiles = proxy.profiles2().await?;
            let active = proxy.active_profile().await?;
            let telemetry = TELEMETRY
                .get_or_init(|| Mutex::new(tuned_rs::telemetry::TelemetryCollector::new()))
                .lock()
                .map_err(|_| anyhow::anyhow!("Telemetry collector lock is poisoned"))?
                .collect()?;
            Ok((
                "application/json; charset=utf-8",
                serde_json::to_vec(&json!({
                    "ok": true,
                    "active": active,
                    "profiles": profiles,
                    "telemetry": telemetry,
                    "cpus": cpu_names(),
                    "networks": network_names(),
                }))?,
            ))
        }
        ("POST", "/api/profile") => {
            let body: ProfileRequest = serde_json::from_slice(&request.body)?;
            validate_token_value(&body.profile)?;
            let connection = zbus::Connection::system().await?;
            let proxy = TunedProxy::new(&connection).await?;
            reply(proxy.switch_profile(&body.profile).await?)
        }
        ("POST", "/api/cpu") => {
            let body: CpuRequest = serde_json::from_slice(&request.body)?;
            validate_devices(&body.devices)?;
            validate_choice(
                &body.governor,
                &[
                    "performance",
                    "powersave",
                    "schedutil",
                    "ondemand",
                    "conservative",
                ],
            )?;
            validate_choice(
                &body.energy_performance_preference,
                &[
                    "performance",
                    "balance_performance",
                    "default",
                    "balance_power",
                    "power",
                ],
            )?;
            let connection = zbus::Connection::system().await?;
            let proxy = TunedProxy::new(&connection).await?;
            let _ = proxy.instance_destroy("tuned-rs-gui-cpu").await;
            reply(
                proxy
                    .instance_create(
                        "cpu",
                        "tuned-rs-gui-cpu",
                        HashMap::from([
                            ("devices".to_string(), body.devices),
                            ("governor".to_string(), body.governor),
                            (
                                "energy_performance_preference".to_string(),
                                body.energy_performance_preference,
                            ),
                        ]),
                    )
                    .await?,
            )
        }
        ("POST", "/api/network") => {
            let body: NetworkRequest = serde_json::from_slice(&request.body)?;
            validate_devices(&body.devices)?;
            if !(576..=65_535).contains(&body.mtu) {
                bail!("MTU must be between 576 and 65535");
            }
            validate_choice(&body.wake_on_lan, &["d", "g", "p", "u", "m", "b", "a", "s"])?;
            let connection = zbus::Connection::system().await?;
            let proxy = TunedProxy::new(&connection).await?;
            let _ = proxy.instance_destroy("tuned-rs-gui-network").await;
            reply(
                proxy
                    .instance_create(
                        "net",
                        "tuned-rs-gui-network",
                        HashMap::from([
                            ("devices".to_string(), body.devices),
                            ("mtu".to_string(), body.mtu.to_string()),
                            ("wake_on_lan".to_string(), body.wake_on_lan),
                        ]),
                    )
                    .await?,
            )
        }
        _ => bail!("Unknown GUI endpoint"),
    }
}

fn reply(result: (bool, String)) -> Result<(&'static str, Vec<u8>)> {
    Ok((
        "application/json; charset=utf-8",
        serde_json::to_vec(&json!({"ok": result.0, "message": result.1}))?,
    ))
}

fn validate_token_value(value: &str) -> Result<()> {
    if !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b' ' | b'.'))
    {
        Ok(())
    } else {
        bail!("Invalid profile value")
    }
}

fn validate_devices(value: &str) -> Result<()> {
    if !value.is_empty()
        && value.len() <= 1024
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'-' | b'.' | b',' | b' ' | b'*' | b'!' | b':')
        })
    {
        Ok(())
    } else {
        bail!("Invalid device selector")
    }
}

fn validate_choice(value: &str, choices: &[&str]) -> Result<()> {
    if choices.contains(&value) {
        Ok(())
    } else {
        bail!("Invalid tuning choice")
    }
}

fn cpu_names() -> Vec<String> {
    names_below("/sys/devices/system/cpu", |name| {
        name.strip_prefix("cpu").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
    })
}

fn network_names() -> Vec<String> {
    names_below("/sys/class/net", |name| name != "lo")
}

fn names_below(path: &str, accept: impl Fn(&str) -> bool) -> Vec<String> {
    let mut names = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| accept(name))
        .collect::<Vec<_>>();
    names.sort();
    names
}

async fn read_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut buffer = Vec::new();
    let header_end = loop {
        if buffer.len() >= MAX_REQUEST {
            bail!("GUI request is too large");
        }
        let mut chunk = [0_u8; 4096];
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            bail!("Incomplete GUI request");
        }
        buffer.extend_from_slice(&chunk[..count]);
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let header = std::str::from_utf8(&buffer[..header_end])?;
    let mut lines = header.lines();
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_string();
    let path = request_line.next().unwrap_or_default().to_string();
    if !matches!(method.as_str(), "GET" | "POST") || !path.starts_with('/') {
        bail!("Invalid GUI HTTP request");
    }
    let mut content_length = 0_usize;
    let mut token = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => content_length = value.trim().parse()?,
            "x-tuned-token" => token = Some(value.trim().to_string()),
            _ => {}
        }
    }
    if header_end + content_length > MAX_REQUEST {
        bail!("GUI request body is too large");
    }
    while buffer.len() < header_end + content_length {
        let mut chunk = [0_u8; 4096];
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            bail!("Incomplete GUI request body");
        }
        buffer.extend_from_slice(&chunk[..count]);
    }
    Ok(HttpRequest {
        method,
        path: path.split('?').next().unwrap_or(&path).to_string(),
        token,
        body: buffer[header_end..header_end + content_length].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_inputs_are_strictly_allowlisted() {
        assert!(validate_devices("eth0").is_ok());
        assert!(validate_devices("../../etc").is_err());
        assert!(validate_choice("performance", &["performance", "powersave"]).is_ok());
        assert!(validate_choice("turbo; reboot", &["performance"]).is_err());
    }
}
