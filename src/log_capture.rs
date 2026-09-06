use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
struct CaptureSession {
    minimum_level: i32,
    expires: Option<Instant>,
    lines: Vec<String>,
    bytes: usize,
    truncated: bool,
}

#[derive(Debug, Default)]
pub struct CaptureStore {
    next_token: AtomicU64,
    sessions: Mutex<HashMap<String, CaptureSession>>,
}

impl CaptureStore {
    pub fn start(&self, minimum_level: i32, timeout_seconds: i32) -> String {
        let sequence = self.next_token.fetch_add(1, Ordering::Relaxed) + 1;
        let token = format!("{sequence:016x}");
        let expires = (timeout_seconds > 0)
            .then(|| Instant::now() + Duration::from_secs(timeout_seconds as u64));
        self.sessions.lock().unwrap().insert(
            token.clone(),
            CaptureSession {
                minimum_level,
                expires,
                lines: Vec::new(),
                bytes: 0,
                truncated: false,
            },
        );
        token
    }

    pub fn finish(&self, token: &str) -> String {
        let Some(session) = self.sessions.lock().unwrap().remove(token) else {
            return String::new();
        };
        if session
            .expires
            .map(|expires| expires <= Instant::now())
            .unwrap_or(false)
        {
            return String::new();
        }
        session.lines.join("\n")
    }

    fn record(&self, level: &Level, line: String) {
        let now = Instant::now();
        let numeric_level = python_log_level(level);
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, session| session.expires.map(|expires| expires > now).unwrap_or(true));

        for session in sessions.values_mut() {
            if numeric_level < session.minimum_level || session.truncated {
                continue;
            }
            let additional = line.len() + usize::from(!session.lines.is_empty());
            if session.bytes.saturating_add(additional) > MAX_CAPTURE_BYTES {
                session.lines.push("[log capture truncated]".to_string());
                session.bytes = MAX_CAPTURE_BYTES;
                session.truncated = true;
                continue;
            }
            session.bytes += additional;
            session.lines.push(line.clone());
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaptureLayer {
    store: Arc<CaptureStore>,
}

impl CaptureLayer {
    pub fn new(store: Arc<CaptureStore>) -> Self {
        Self { store }
    }
}

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let line = format!(
            "{timestamp} {} {} {}",
            metadata.level(),
            metadata.target(),
            visitor.output
        );
        self.store.record(metadata.level(), line);
    }
}

#[derive(Debug, Default)]
struct FieldVisitor {
    output: String,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if !self.output.is_empty() {
            self.output.push(' ');
        }
        let _ = write!(&mut self.output, "{}={value:?}", field.name());
    }
}

fn python_log_level(level: &Level) -> i32 {
    match *level {
        Level::ERROR => 40,
        Level::WARN => 30,
        Level::INFO => 20,
        Level::DEBUG => 10,
        Level::TRACE => 5,
    }
}

static GLOBAL_CAPTURE: OnceLock<Arc<CaptureStore>> = OnceLock::new();

pub fn global_store() -> Arc<CaptureStore> {
    GLOBAL_CAPTURE
        .get_or_init(|| Arc::new(CaptureStore::default()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_respects_python_logging_thresholds() {
        let store = CaptureStore::default();
        let token = store.start(20, 0);
        store.record(&Level::DEBUG, "debug".to_string());
        store.record(&Level::INFO, "info".to_string());
        store.record(&Level::ERROR, "error".to_string());
        assert_eq!(store.finish(&token), "info\nerror");
    }

    #[test]
    fn capture_tokens_are_unique_and_one_shot() {
        let store = CaptureStore::default();
        let first = store.start(10, 0);
        let second = store.start(10, 0);
        assert_ne!(first, second);
        store.record(&Level::INFO, "event".to_string());
        assert_eq!(store.finish(&first), "event");
        assert_eq!(store.finish(&first), "");
        assert_eq!(store.finish(&second), "event");
    }

    #[test]
    fn expired_capture_returns_empty() {
        let store = CaptureStore::default();
        let token = store.start(10, 1);
        {
            let mut sessions = store.sessions.lock().unwrap();
            sessions.get_mut(&token).unwrap().expires = Some(Instant::now());
        }
        assert_eq!(store.finish(&token), "");
    }
}
