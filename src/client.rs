//! Minimal JSON-RPC 2.0 client over HTTP (ureq). Sends the account API token as X-Fdb-User-Token,
//! reads the X-Fdb-Resources accounting header, and maps HTTP-level refusals (429 rate/quota, 403 blocked) to typed errors that carry distinct exit codes.

use std::fmt;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Every way a command can fail, with the process exit code it maps to.
#[derive(Debug)]
pub enum Error {
    /// The method returned a JSON-RPC error object (exit 1).
    Rpc { code: i64, message: String },
    /// The call succeeded but reported a negative outcome, e.g. a rejected login (exit 1).
    Failed(String),
    /// Bad command-line input (exit 2).
    Usage(String),
    /// Unreachable service, malformed reply, or any other non-2xx HTTP status (exit 3).
    Transport(String),
    /// HTTP 429: request rate or a resource quota exhausted (exit 4).
    Limited { message: String, retry_after: Option<u64> },
    /// HTTP 403: this client address is blocked (exit 5).
    Blocked(String),
    /// Local I/O or config problem (exit 1).
    Other(anyhow::Error),
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Rpc { .. } | Error::Failed(_) | Error::Other(_) => 1,
            Error::Usage(_) => 2,
            Error::Transport(_) => 3,
            Error::Limited { .. } => 4,
            Error::Blocked(_) => 5,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Rpc { code, message } => write!(f, "rpc error {code}: {message}"),
            Error::Failed(m) => write!(f, "{m}"),
            Error::Usage(m) => write!(f, "{m}"),
            Error::Transport(m) => write!(f, "{m}"),
            Error::Limited { message, retry_after } => match retry_after {
                Some(s) => write!(f, "{message} (retry after {s}s)"),
                None => write!(f, "{message}"),
            },
            Error::Blocked(m) => write!(f, "blocked: {m}"),
            Error::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Other(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Other(e.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// One method invocation.
pub struct Call {
    pub method: String,
    pub params: Value,
}

pub struct Client {
    agent: ureq::Agent,
    url: String,
    token: Option<String>,
    verbose: bool,
    /// Timeout for ordinary (read) calls.
    timeout: Option<Duration>,
    /// Timeout for calls that do long server-side work (writes such as `prove`, `extend_sequence`,
    /// certificate uploads). A client that gives up early is misleading: the server still finishes.
    long_timeout: Option<Duration>,
}

impl Client {
    pub fn new(url: String, token: Option<String>, timeout: Option<Duration>, long_timeout: Option<Duration>, verbose: bool) -> Self {
        let agent = ureq::Agent::config_builder()
            // Keep 4xx/5xx as responses so their bodies (the 429 quota message) can be read.
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            .user_agent(concat!("fdb-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        Client { agent, url, token, verbose, timeout, long_timeout }
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// A single call: the `result` payload, or the error the method returned.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let mut out = self.send(&[Call { method: method.to_string(), params }], false, false)?;
        out.pop().unwrap_or(Err(Error::Transport("empty response".into())))
    }

    /// A single call that may take long on the server (a write); uses the long timeout.
    pub fn call_long(&self, method: &str, params: Value) -> Result<Value> {
        let mut out = self.send(&[Call { method: method.to_string(), params }], false, true)?;
        out.pop().unwrap_or(Err(Error::Transport("empty response".into())))
    }

    /// A batch in one round-trip: per-item outcomes in request order. Only transport / protocol
    /// failures fail the whole batch.
    pub fn batch(&self, calls: &[Call]) -> Result<Vec<Result<Value>>> {
        self.send(calls, true, true)
    }

    fn send(&self, calls: &[Call], as_array: bool, long: bool) -> Result<Vec<Result<Value>>> {
        let items: Vec<Value> = calls
            .iter()
            .enumerate()
            .map(|(i, c)| json!({ "jsonrpc": "2.0", "id": i, "method": c.method, "params": c.params }))
            .collect();
        let payload = if as_array { Value::Array(items) } else { items.into_iter().next().unwrap_or(Value::Null) };
        let body = serde_json::to_vec(&payload).map_err(|e| Error::Transport(format!("cannot encode request: {e}")))?;

        if self.verbose {
            for c in calls {
                eprintln!("> {} {}", c.method, scrub(&c.params));
            }
        }
        let t0 = Instant::now();
        let timeout = if long { self.long_timeout } else { self.timeout };
        let mut req = self
            .agent
            .post(&self.url)
            .config()
            .timeout_global(timeout)
            .build()
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
        if let Some(t) = &self.token {
            if !t.is_empty() {
                req = req.header("X-Fdb-User-Token", t);
            }
        }
        let mut resp = req.send(&body[..]).map_err(|e| Error::Transport(format!("{}: {}", self.url, describe(e))))?;
        let status = resp.status().as_u16();
        let resources = resp.headers().get("x-fdb-resources").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.trim().parse().ok());
        // No body cap: a certificate download or a full decimal expansion can run to hundreds of MB.
        let text = resp
            .body_mut()
            .with_config()
            .limit(u64::MAX)
            .read_to_string()
            .map_err(|e| Error::Transport(format!("reading response: {}", describe(e))))?;
        if self.verbose {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            match &resources {
                Some(r) => eprintln!("< HTTP {status} in {ms:.1} ms; {r}"),
                None => eprintln!("< HTTP {status} in {ms:.1} ms"),
            }
        }
        match status {
            200..=299 => {}
            429 => {
                let msg = text.trim();
                let message = if msg.is_empty() { "rate limit exceeded".to_string() } else { msg.to_string() };
                return Err(Error::Limited { message, retry_after });
            }
            403 => return Err(Error::Blocked(text.trim().to_string())),
            _ => {
                let snippet: String = text.trim().chars().take(300).collect();
                return Err(Error::Transport(format!("HTTP {status} from {}: {snippet}", self.url)));
            }
        }
        let decoded: Value = serde_json::from_str(&text).map_err(|e| {
            let snippet: String = text.trim().chars().take(200).collect();
            Error::Transport(format!("unexpected non-JSON response: {e}  {snippet}"))
        })?;
        let list: Vec<Value> = match decoded {
            Value::Array(a) => a,
            v => vec![v],
        };
        // Match replies to requests by the numeric id we sent (0..n-1).
        let mut by_id: Vec<Option<Value>> = vec![None; calls.len()];
        for r in list {
            let id = r.get("id").and_then(Value::as_u64).map(|i| i as usize);
            match id {
                Some(i) if i < by_id.len() && by_id[i].is_none() => by_id[i] = Some(r),
                _ => {
                    // A reply with a null/unknown id is a protocol-level failure (e.g. a parse error).
                    if let Some(err) = r.get("error") {
                        return Err(Error::Transport(format!(
                            "protocol error {}: {}",
                            err.get("code").and_then(Value::as_i64).unwrap_or(0),
                            err.get("message").and_then(Value::as_str).unwrap_or("?")
                        )));
                    }
                    return Err(Error::Transport("response item with missing or duplicate id".into()));
                }
            }
        }
        Ok(by_id
            .into_iter()
            .map(|r| match r {
                None => Err(Error::Transport("missing response".into())),
                Some(r) => match r.get("error") {
                    Some(e) => Err(Error::Rpc {
                        code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: e.get("message").and_then(Value::as_str).unwrap_or("RPC error").to_string(),
                    }),
                    None => Ok(r.get("result").cloned().unwrap_or(Value::Null)),
                },
            })
            .collect())
    }
}

/// A short human description of a ureq failure (connection refused, timeout, DNS).
fn describe(e: ureq::Error) -> String {
    match e {
        ureq::Error::Io(io) => match io.kind() {
            std::io::ErrorKind::ConnectionRefused => "connection refused (is fdb-rpc running?)".to_string(),
            _ => io.to_string(),
        },
        ureq::Error::Timeout(t) => {
            format!("timed out ({t}); the server may still complete the request, raise --timeout, or pass --timeout 0 for none")
        }
        ureq::Error::HostNotFound => "host not found".to_string(),
        ureq::Error::ConnectionFailed => "connection failed".to_string(),
        other => other.to_string(),
    }
}

/// Params for the verbose log: secrets hidden, long strings (certificate bodies) shortened.
fn scrub(v: &Value) -> String {
    fn walk(v: &Value) -> Value {
        match v {
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, x)| {
                        let kl = k.to_ascii_lowercase();
                        let hidden = matches!(kl.as_str(), "pass" | "password" | "session" | "token");
                        (k.clone(), if hidden { Value::String(" hidden ".into()) } else { walk(x) })
                    })
                    .collect(),
            ),
            Value::Array(a) => Value::Array(a.iter().map(walk).collect()),
            Value::String(s) if s.chars().count() > 120 => {
                Value::String(format!("{} ({} chars)", s.chars().take(120).collect::<String>(), s.chars().count()))
            }
            other => other.clone(),
        }
    }
    walk(v).to_string()
}

/// Parse a command-line target: id:N, fid:N or #N address a stored id; anything else is an expression (2^127-1, 150!+1, a decimal number).
pub fn target(s: &str) -> Result<Value> {
    let t = s.trim();
    for prefix in ["id:", "fid:", "#"] {
        if let Some(rest) = t.strip_prefix(prefix) {
            return match rest.trim().parse::<u64>() {
                Ok(n) => Ok(json!({ "id": n })),
                Err(_) => Err(Error::Usage(format!("'{s}': an id must be a non-negative integer after '{prefix}'"))),
            };
        }
    }
    if t.is_empty() {
        return Err(Error::Usage("empty target".into()));
    }
    Ok(json!({ "expr": t }))
}
