use axum::http::{HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Api {
    Openai,
    Anthropic,
    Responses,
    Embeddings,
}

impl Api {
    pub fn provider(self) -> &'static str {
        if self == Self::Anthropic {
            "anthropic"
        } else {
            "openai"
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub listen: SocketAddr,
    pub inbound_auth: InboundAuth,
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default)]
    pub routing: Routing,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub transport: Transport,
    #[serde(deserialize_with = "unique_map")]
    pub backends: BTreeMap<String, BackendConfig>,
    #[serde(deserialize_with = "unique_map")]
    pub models: BTreeMap<String, ModelConfig>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum InboundAuth {
    None,
    Keys { key_envs: Vec<String> },
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Runtime {
    pub worker_threads: Option<usize>,
}
impl Runtime {
    pub fn threads(&self) -> usize {
        self.worker_threads
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get().min(8)))
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Routing {
    pub session_ttl_secs: u64,
    pub state_ttl_secs: u64,
    pub max_sessions_per_model: usize,
    pub max_state_entries: usize,
    pub strict_file_ownership: bool,
    pub user_id_fallback: bool,
}
impl Default for Routing {
    fn default() -> Self {
        Self {
            session_ttl_secs: 3600,
            state_ttl_secs: 86400,
            max_sessions_per_model: 10000,
            max_state_entries: 100000,
            strict_file_ownership: true,
            user_id_fallback: false,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_open_requests: usize,
    pub max_json_bytes: usize,
    pub buffered_json_budget_bytes: usize,
    pub max_upload_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_open_requests: 1024,
            max_json_bytes: 64 * 1024 * 1024,
            buffered_json_budget_bytes: 256 * 1024 * 1024,
            max_upload_bytes: 512 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Transport {
    pub connect_timeout_ms: u64,
    pub header_timeout_ms: Option<u64>,
    pub body_idle_timeout_ms: Option<u64>,
    pub json_body_idle_timeout_ms: Option<u64>,
    pub pool_idle_timeout_secs: u64,
    pub pool_max_idle_per_host: usize,
    pub health_interval_ms: u64,
    pub health_timeout_ms: u64,
    pub shutdown_timeout_secs: u64,
}
impl Default for Transport {
    fn default() -> Self {
        Self {
            connect_timeout_ms: 10000,
            header_timeout_ms: None,
            body_idle_timeout_ms: None,
            json_body_idle_timeout_ms: Some(60_000),
            pool_idle_timeout_secs: 90,
            pool_max_idle_per_host: 128,
            health_interval_ms: 10000,
            health_timeout_ms: 2000,
            shutdown_timeout_secs: 30,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    pub base_url: String,
    pub apis: Vec<Api>,
    #[serde(default = "capacity")]
    pub capacity: usize,
    #[serde(default = "cooldown")]
    pub cooldown_ms: u64,
    #[serde(default)]
    pub auth: BackendAuth,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub health_path: Option<String>,
    pub queue_path: Option<String>,
}
fn capacity() -> usize {
    64
}
fn cooldown() -> u64 {
    3000
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackendAuth {
    #[serde(default)]
    pub scheme: AuthScheme,
    pub env: Option<String>,
}
#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    #[default]
    None,
    Auto,
    Bearer,
    #[serde(alias = "x-api-key")]
    XApiKey,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    #[serde(default)]
    pub allow_raw: bool,
    pub backends: Vec<Binding>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub backend: String,
    pub model: String,
}

pub fn secret(name: &str) -> Result<String> {
    std::env::var(name)
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("required environment variable {name} is not set"))
}

pub fn reserved_header(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "x-api-key"
            | "api-key"
            | "proxy-authorization"
            | "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "upgrade"
            | "proxy-authenticate"
    ) || name.starts_with("x-apibond-")
}

impl Config {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| format!("read config: {e}"))?;
        let config: Self =
            serde_json::from_slice(&bytes).map_err(|e| format!("invalid config: {e}"))?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err("schema_version must be 1".into());
        }
        if self.runtime.worker_threads == Some(0) || self.runtime.threads() > 512 {
            return Err("runtime.worker_threads must be 1..512 or null".into());
        }
        if let InboundAuth::Keys { key_envs } = &self.inbound_auth {
            if key_envs.is_empty() {
                return Err("inbound_auth.keys requires key_envs".into());
            }
            for name in key_envs {
                secret(name)?;
            }
        }
        let l = &self.limits;
        let r = &self.routing;
        let t = &self.transport;
        if l.max_open_requests == 0
            || l.max_open_requests > tokio::sync::Semaphore::MAX_PERMITS
            || l.max_json_bytes == 0
            || l.max_upload_bytes == 0
            || l.buffered_json_budget_bytes > u32::MAX as usize
            || l.max_json_bytes > l.buffered_json_budget_bytes / 2
        {
            return Err("invalid limits: positive values and JSON budget >= 2 * max_json_bytes (<= u32::MAX) required".into());
        }
        if r.max_sessions_per_model <= l.max_open_requests
            || r.max_state_entries == 0
            || r.session_ttl_secs == 0
            || r.state_ttl_secs == 0
        {
            return Err(
                "invalid routing limits; max_sessions_per_model must exceed max_open_requests"
                    .into(),
            );
        }
        if t.connect_timeout_ms == 0
            || t.health_timeout_ms == 0
            || t.health_interval_ms == 0
            || t.shutdown_timeout_secs == 0
            || t.header_timeout_ms == Some(0)
            || t.body_idle_timeout_ms == Some(0)
            || t.json_body_idle_timeout_ms == Some(0)
        {
            return Err("timeouts must be positive or null".into());
        }
        if self.backends.is_empty() || self.models.is_empty() {
            return Err("backends and models must not be empty".into());
        }
        for (id, b) in &self.backends {
            valid_id(id)?;
            let url = reqwest::Url::parse(&b.base_url)
                .map_err(|_| format!("backend {id}: invalid base_url"))?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || b.capacity == 0
                || b.apis.is_empty()
            {
                return Err(format!("backend {id}: invalid URL/capacity/apis"));
            }
            for path in [&b.health_path, &b.queue_path].into_iter().flatten() {
                if !path.starts_with('/') || path.starts_with("//") || path.contains(['#', '?']) {
                    return Err(format!("backend {id}: probe paths must be origin-relative"));
                }
            }
            match b.auth.scheme {
                AuthScheme::None if b.auth.env.is_some() => {
                    return Err(format!("backend {id}: auth none must not specify env"));
                }
                AuthScheme::None => (),
                _ => {
                    let name = b
                        .auth
                        .env
                        .as_deref()
                        .ok_or_else(|| format!("backend {id}: auth requires env"))?;
                    HeaderValue::from_str(&secret(name)?)
                        .map_err(|_| format!("backend {id}: invalid credential header"))?;
                }
            }
            for (name, value) in &b.headers {
                let n = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| format!("backend {id}: invalid header name"))?;
                if reserved_header(n.as_str()) {
                    return Err(format!("backend {id}: reserved custom header {name}"));
                }
                HeaderValue::from_str(value)
                    .map_err(|_| format!("backend {id}: invalid header value"))?;
            }
        }
        for (alias, m) in &self.models {
            valid_id(alias)?;
            if m.backends.is_empty() {
                return Err(format!("model {alias}: empty pool"));
            }
            let mut seen = std::collections::HashSet::new();
            for binding in &m.backends {
                if !self.backends.contains_key(&binding.backend)
                    || !seen.insert(&binding.backend)
                    || binding.model.is_empty()
                    || binding.model.len() > 512
                {
                    return Err(format!(
                        "model {alias}: unknown/duplicate backend or invalid upstream model"
                    ));
                }
                if m.allow_raw && binding.model != m.backends[0].model {
                    return Err(format!(
                        "model {alias}: raw requires one common upstream model"
                    ));
                }
            }
        }
        Ok(())
    }
}

pub fn valid_id(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 512 || s.chars().any(char::is_control) {
        Err("IDs must contain 1..512 bytes and no control characters".into())
    } else {
        Ok(())
    }
}

fn unique_map<'de, D, T>(d: D) -> std::result::Result<BTreeMap<String, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Unique<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Unique<T> {
        type Value = BTreeMap<String, T>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("map with unique names")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut m: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = m.next_entry::<String, T>()? {
                if result.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate configuration name"));
                }
            }
            Ok(result)
        }
    }
    d.deserialize_map(Unique(std::marker::PhantomData))
}
