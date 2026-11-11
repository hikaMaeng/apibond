use crate::config::{Api, AuthScheme, Config, InboundAuth, Result, secret};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use lru::LruCache;
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

pub type Scope = [u8; 32];
type SessionKey = (Scope, String);
type HandleKey = (Scope, String, String);

pub struct Backend {
    pub id: String,
    pub config: crate::config::BackendConfig,
    pub active: AtomicUsize,
    pub queue: AtomicUsize,
    pub ready: AtomicBool,
    cooldown: Mutex<Option<Instant>>,
    pub secret: Option<String>,
    pub headers: HeaderMap,
}
impl Backend {
    pub fn available(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
            && self.cooldown.lock().is_none_or(|t| t <= Instant::now())
    }
    pub fn failed_connect(&self) {
        *self.cooldown.lock() =
            Some(Instant::now() + Duration::from_millis(self.config.cooldown_ms));
    }
    fn reserve(self: &Arc<Self>) -> Option<BackendGuard> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |n| {
                (n < self.config.capacity).then_some(n + 1)
            })
            .ok()
            .map(|_| BackendGuard(self.clone()))
    }
    pub fn url(&self, path_query: &str) -> String {
        format!(
            "{}{}",
            self.config.base_url.trim_end_matches('/'),
            path_query.strip_prefix("/v1").unwrap_or(path_query)
        )
    }
    pub fn auth_headers(&self, api: Api, headers: &mut HeaderMap) {
        if let Some(secret) = &self.secret {
            let scheme = match self.config.auth.scheme {
                AuthScheme::Auto if api == Api::Anthropic => AuthScheme::XApiKey,
                AuthScheme::Auto => AuthScheme::Bearer,
                s => s,
            };
            let (name, value) = if scheme == AuthScheme::XApiKey {
                ("x-api-key", secret.clone())
            } else {
                ("authorization", format!("Bearer {secret}"))
            };
            let mut value = HeaderValue::from_str(&value).expect("credential validated");
            value.set_sensitive(true);
            headers.insert(HeaderName::from_static(name), value);
        }
        for (name, value) in &self.headers {
            headers.insert(name.clone(), value.clone());
        }
    }
}
pub struct BackendGuard(Arc<Backend>);
impl Drop for BackendGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct Binding {
    pub backend: usize,
    pub model: String,
}
pub struct Pool {
    pub alias: String,
    pub allow_raw: bool,
    pub bindings: Vec<Binding>,
    sessions: Mutex<Sessions>,
}
struct Sessions {
    entries: LruCache<SessionKey, Session>,
    counts: HashMap<usize, usize>,
    cursor: usize,
}
impl Sessions {
    fn new(capacity: usize) -> Self {
        Self {
            entries: LruCache::new(NonZeroUsize::new(capacity).unwrap()),
            counts: HashMap::new(),
            cursor: 0,
        }
    }
    fn remove(&mut self, key: &SessionKey) {
        if let Some(s) = self.entries.pop(key)
            && let Some(n) = self.counts.get_mut(&s.backend)
        {
            *n = n.saturating_sub(1);
        }
    }
}
struct Session {
    backend: usize,
    last: Instant,
    active: usize,
}

#[derive(Clone)]
pub struct Owner {
    pub alias: String,
    pub backend: usize,
    last: Instant,
    ambiguous: bool,
}

pub struct App {
    pub config: Config,
    pub client: reqwest::Client,
    pub backends: Vec<Arc<Backend>>,
    pub pools: BTreeMap<String, Arc<Pool>>,
    pub requests: Arc<Semaphore>,
    pub buffer: Arc<Semaphore>,
    keys: Vec<Scope>,
    states: Mutex<LruCache<HandleKey, Owner>>,
}

#[derive(Debug)]
pub struct RouteError {
    pub status: u16,
    pub message: String,
}
impl RouteError {
    pub fn new(status: u16, msg: impl Into<String>) -> Self {
        Self {
            status,
            message: msg.into(),
        }
    }
}

pub struct Selection {
    pub backend: Arc<Backend>,
    pub upstream_model: String,
    _capacity: BackendGuard,
    session: Option<(Arc<Pool>, SessionKey)>,
}
impl Drop for Selection {
    fn drop(&mut self) {
        if let Some((pool, key)) = &self.session
            && let Some(entry) = pool.sessions.lock().entries.get_mut(key)
        {
            entry.active = entry.active.saturating_sub(1);
            entry.last = Instant::now();
        }
    }
}

pub struct Route<'a> {
    pub alias: &'a str,
    pub api: Api,
    pub scope: Scope,
    pub session: Option<&'a str>,
    pub explicit: Option<&'a str>,
    pub owners: Vec<Owner>,
    pub needs_pin: bool,
}

impl App {
    pub fn new(config: Config) -> Result<Arc<Self>> {
        config.validate()?;
        let mut keys = Vec::new();
        if let InboundAuth::Keys { key_envs } = &config.inbound_auth {
            for name in key_envs {
                keys.push(Sha256::digest(secret(name)?.as_bytes()).into());
            }
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_millis(config.transport.connect_timeout_ms))
            .pool_idle_timeout(Duration::from_secs(config.transport.pool_idle_timeout_secs))
            .pool_max_idle_per_host(config.transport.pool_max_idle_per_host)
            .tcp_nodelay(true)
            .build()
            .map_err(|e| format!("HTTP client: {e}"))?;
        let mut backends = Vec::new();
        let mut indices = HashMap::new();
        for (id, b) in &config.backends {
            indices.insert(id.clone(), backends.len());
            let mut headers = HeaderMap::new();
            for (k, v) in &b.headers {
                headers.insert(
                    HeaderName::from_bytes(k.as_bytes()).unwrap(),
                    HeaderValue::from_str(v).unwrap(),
                );
            }
            backends.push(Arc::new(Backend {
                id: id.clone(),
                config: b.clone(),
                active: AtomicUsize::new(0),
                queue: AtomicUsize::new(0),
                ready: AtomicBool::new(b.health_path.is_none()),
                cooldown: Mutex::new(None),
                secret: b.auth.env.as_deref().map(secret).transpose()?,
                headers,
            }));
        }
        let pools = config
            .models
            .iter()
            .map(|(alias, m)| {
                (
                    alias.clone(),
                    Arc::new(Pool {
                        alias: alias.clone(),
                        allow_raw: m.allow_raw,
                        bindings: m
                            .backends
                            .iter()
                            .map(|b| Binding {
                                backend: indices[&b.backend],
                                model: b.model.clone(),
                            })
                            .collect(),
                        sessions: Mutex::new(Sessions::new(config.routing.max_sessions_per_model)),
                    }),
                )
            })
            .collect();
        let requests = Arc::new(Semaphore::new(config.limits.max_open_requests));
        let buffer = Arc::new(Semaphore::new(config.limits.buffered_json_budget_bytes));
        let states = Mutex::new(LruCache::new(
            NonZeroUsize::new(config.routing.max_state_entries).unwrap(),
        ));
        Ok(Arc::new(Self {
            config,
            client,
            backends,
            pools,
            requests,
            buffer,
            keys,
            states,
        }))
    }
    pub fn authenticate(&self, headers: &HeaderMap) -> std::result::Result<Scope, RouteError> {
        let auth = headers.get_all("authorization").iter().collect::<Vec<_>>();
        let api = headers.get_all("x-api-key").iter().collect::<Vec<_>>();
        if auth.len() + api.len() > 1 {
            return Err(RouteError::new(401, "supply only one inbound credential"));
        }
        if self.keys.is_empty() {
            return Ok([0; 32]);
        }
        let credential = if let Some(v) = auth.first() {
            v.to_str().ok().and_then(|s| s.strip_prefix("Bearer "))
        } else {
            api.first().and_then(|v| v.to_str().ok())
        };
        let candidate: Scope = Sha256::digest(credential.unwrap_or("").as_bytes()).into();
        let mut matched = 0u8;
        for key in &self.keys {
            matched |= key.ct_eq(&candidate).unwrap_u8();
        }
        if matched == 1 && credential.is_some() {
            Ok(candidate)
        } else {
            Err(RouteError::new(
                401,
                "invalid or missing APIBond credential",
            ))
        }
    }
    pub fn owner(&self, scope: Scope, provider: &str, id: &str) -> Option<Owner> {
        let mut states = self.states.lock();
        let key = (scope, provider.to_string(), id.to_string());
        let entry = states.get_mut(&key)?;
        if entry.last.elapsed() > Duration::from_secs(self.config.routing.state_ttl_secs) {
            states.pop(&key);
            return None;
        }
        entry.last = Instant::now();
        if entry.ambiguous {
            return None;
        }
        Some(entry.clone())
    }
    pub fn remember(&self, scope: Scope, provider: &str, id: String, alias: &str, backend: usize) {
        let mut states = self.states.lock();
        let key = (scope, provider.to_string(), id);
        if let Some(existing) = states.get_mut(&key) {
            existing.ambiguous |= existing.backend != backend;
            existing.last = Instant::now();
            return;
        }
        states.put(
            key,
            Owner {
                alias: alias.to_string(),
                backend,
                last: Instant::now(),
                ambiguous: false,
            },
        );
    }
    pub fn select(&self, r: Route<'_>) -> std::result::Result<Selection, RouteError> {
        let pool = self
            .pools
            .get(r.alias)
            .ok_or_else(|| RouteError::new(404, "unknown public model"))?;
        let mut sessions = pool.sessions.lock();
        let key = r.session.map(|s| (r.scope, s.to_string()));
        if let Some(k) = &key {
            let expire = sessions.entries.get(k).is_some_and(|s| {
                s.active == 0
                    && s.last.elapsed() > Duration::from_secs(self.config.routing.session_ttl_secs)
            });
            if expire {
                sessions.remove(k);
            }
        }
        let pinned = key
            .as_ref()
            .and_then(|k| sessions.entries.get(k))
            .map(|s| s.backend);
        let explicit = r
            .explicit
            .map(|id| {
                self.backends
                    .iter()
                    .position(|b| b.id == id)
                    .ok_or_else(|| RouteError::new(400, "unknown backend ID"))
            })
            .transpose()?;
        let mut forced = None;
        for owner in &r.owners {
            if forced.is_some_and(|b| b != owner.backend) {
                return Err(RouteError::new(
                    409,
                    "state handles belong to different backends",
                ));
            }
            forced = Some(owner.backend);
        }
        for candidate in [explicit, pinned].into_iter().flatten() {
            if forced.is_some_and(|b| b != candidate) {
                return Err(RouteError::new(
                    409,
                    "state owner conflicts with session or explicit backend",
                ));
            }
            if !r.owners.is_empty() || explicit.is_some() {
                forced = Some(candidate);
            }
        }
        if r.needs_pin && forced.is_none() {
            forced = explicit.or(pinned);
            if forced.is_none() {
                return Err(RouteError::new(
                    409,
                    "unknown state owner; supply an existing session or x-apibond-backend",
                ));
            }
        }
        let mut preferred = forced.or(pinned);
        if let Some(index) = preferred {
            let binding = pool
                .bindings
                .iter()
                .find(|b| b.backend == index)
                .ok_or_else(|| RouteError::new(409, "owner is outside the selected model pool"))?;
            let b = &self.backends[binding.backend];
            if !b.config.apis.contains(&r.api) {
                return Err(RouteError::new(
                    409,
                    "pinned backend does not support this API family",
                ));
            }
            if !b.available() {
                if forced.is_some() || r.needs_pin || !r.owners.is_empty() {
                    return Err(RouteError::new(503, "state owner is unavailable"));
                }
                if let Some(k) = &key {
                    if sessions.entries.get(k).is_some_and(|s| s.active > 0) {
                        return Err(RouteError::new(
                            503,
                            "session still has requests on unavailable backend",
                        ));
                    }
                    sessions.remove(k);
                }
                preferred = None;
            }
        }
        let mut candidates = pool
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| {
                let b = &self.backends[binding.backend];
                b.config.apis.contains(&r.api)
                    && b.available()
                    && preferred.is_none_or(|p| p == binding.backend)
            })
            .map(|(position, binding)| {
                let backend = &self.backends[binding.backend];
                (
                    position,
                    binding,
                    backend.active.load(Ordering::Relaxed) + backend.queue.load(Ordering::Relaxed),
                    sessions.counts.get(&binding.backend).copied().unwrap_or(0),
                )
            })
            .collect::<Vec<_>>();
        let count = pool.bindings.len();
        let cursor = sessions.cursor;
        candidates.sort_by(|(ai, a, load_a, sa), (bi, b, load_b, sb)| {
            let ab = &self.backends[a.backend];
            let bb = &self.backends[b.backend];
            let compare = (*load_a as u128 * bb.config.capacity as u128)
                .cmp(&(*load_b as u128 * ab.config.capacity as u128));
            compare
                .then_with(|| {
                    (*sa as u128 * bb.config.capacity as u128)
                        .cmp(&(*sb as u128 * ab.config.capacity as u128))
                })
                .then_with(|| {
                    ((ai + count - cursor % count) % count)
                        .cmp(&((bi + count - cursor % count) % count))
                })
        });
        for (position, binding, _, _) in candidates {
            if let Some(capacity) = self.backends[binding.backend].reserve() {
                if let Some(k) = &key {
                    if !sessions.entries.contains(k)
                        && sessions.entries.len() >= self.config.routing.max_sessions_per_model
                    {
                        let evict = sessions
                            .entries
                            .iter()
                            .rev()
                            .find(|(_, s)| s.active == 0)
                            .map(|(k, _)| k.clone());
                        if let Some(old) = evict {
                            sessions.remove(&old);
                        } else {
                            return Err(RouteError::new(503, "session table is full"));
                        }
                    }
                    if !sessions.entries.contains(k) {
                        *sessions.counts.entry(binding.backend).or_default() += 1;
                        sessions.entries.put(
                            k.clone(),
                            Session {
                                backend: binding.backend,
                                last: Instant::now(),
                                active: 0,
                            },
                        );
                    }
                    let entry = sessions.entries.get_mut(k).unwrap();
                    entry.active += 1;
                    entry.last = Instant::now();
                }
                sessions.cursor = (position + 1) % count;
                return Ok(Selection {
                    backend: self.backends[binding.backend].clone(),
                    upstream_model: binding.model.clone(),
                    _capacity: capacity,
                    session: key.map(|k| (pool.clone(), k)),
                });
            }
        }
        Err(RouteError::new(
            503,
            "no eligible backend has free capacity",
        ))
    }
    pub fn models(&self, anthropic: bool) -> Value {
        let data=self.pools.values().map(|p|{
            let apis=p.bindings.iter().flat_map(|b|self.backends[b.backend].config.apis.iter().copied()).collect::<std::collections::HashSet<_>>();
            let mut value=if anthropic {json!({"id":p.alias,"type":"model","display_name":p.alias,"created_at":"2026-11-11T00:00:00Z"})} else {json!({"id":p.alias,"object":"model","created":0,"owned_by":"apibond"})};
            let mut apis=apis.into_iter().map(|a|serde_json::to_value(a).unwrap()).collect::<Vec<_>>();apis.sort_by_key(Value::to_string);
            value["apibond"]=json!({"apis":apis,"raw_enabled":p.allow_raw,"raw_upstream_model":if p.allow_raw {Some(&p.bindings[0].model)}else{None}});value
        }).collect::<Vec<_>>();
        if anthropic {
            json!({"data":data,"has_more":false,"first_id":data.first().and_then(|v|v.get("id")),"last_id":data.last().and_then(|v|v.get("id"))})
        } else {
            json!({"object":"list","data":data})
        }
    }
    pub fn status(&self) -> Value {
        json!({"backends":self.backends.iter().map(|b|json!({"id":b.id,"available":b.available(),"active":b.active.load(Ordering::Relaxed),"queued":b.queue.load(Ordering::Relaxed),"capacity":b.config.capacity})).collect::<Vec<_>>(),"models":self.pools.iter().map(|(id,p)|(id,p.sessions.lock().entries.len())).collect::<BTreeMap<_,_>>(),"state_entries":self.states.lock().len(),"buffer_bytes_reserved":self.config.limits.buffered_json_budget_bytes-self.buffer.available_permits()})
    }
    pub fn spawn_maintenance(app: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(app);
        let interval = app.config.transport.health_interval_ms;
        tokio::spawn(async move {
            loop {
                let Some(app) = weak.upgrade() else {
                    break;
                };
                for backend in &app.backends {
                    for (path, is_queue) in [
                        (&backend.config.health_path, false),
                        (&backend.config.queue_path, true),
                    ] {
                        let Some(path) = path else {
                            continue;
                        };
                        let mut url = reqwest::Url::parse(&backend.config.base_url).unwrap();
                        url.set_path(path);
                        let mut headers = HeaderMap::new();
                        backend.auth_headers(backend.config.apis[0], &mut headers);
                        let response = app
                            .client
                            .get(url)
                            .headers(headers)
                            .timeout(Duration::from_millis(
                                app.config.transport.health_timeout_ms,
                            ))
                            .send()
                            .await;
                        if is_queue {
                            if let Ok(res) = response
                                && res.status().is_success()
                                && res.content_length().is_some_and(|n| n <= 16384)
                                && let Ok(bytes) = res.bytes().await
                                && let Ok(v) = serde_json::from_slice::<Value>(&bytes)
                                && let Some(n) = v.get("queue").and_then(Value::as_u64)
                            {
                                backend.queue.store(
                                    n.min(usize::MAX as u64 / 4) as usize,
                                    Ordering::Relaxed,
                                );
                            }
                        } else {
                            backend.ready.store(
                                response.is_ok_and(|r| r.status().is_success()),
                                Ordering::Relaxed,
                            );
                        }
                    }
                }
                let session_ttl = Duration::from_secs(app.config.routing.session_ttl_secs);
                for pool in app.pools.values() {
                    let mut sessions = pool.sessions.lock();
                    let expired = sessions
                        .entries
                        .iter()
                        .filter(|(_, s)| s.active == 0 && s.last.elapsed() >= session_ttl)
                        .map(|(k, _)| k.clone())
                        .collect::<Vec<_>>();
                    for key in expired {
                        sessions.remove(&key);
                    }
                }
                let state_ttl = Duration::from_secs(app.config.routing.state_ttl_secs);
                {
                    let mut states = app.states.lock();
                    while states
                        .peek_lru()
                        .is_some_and(|(_, s)| s.last.elapsed() >= state_ttl)
                    {
                        states.pop_lru();
                    }
                }
                drop(app);
                tokio::time::sleep(Duration::from_millis(interval)).await;
            }
        })
    }
}
