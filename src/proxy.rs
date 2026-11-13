use crate::{
    config::{Api, valid_id},
    inspect::{self, IdProbe, Metadata},
    routing::{App, Route, RouteError},
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::BytesMut;
use futures_util::StreamExt;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub fn router(app: Arc<App>) -> Router {
    Router::new().fallback(handle).with_state(app)
}

fn error(api: Api, e: RouteError) -> Response {
    let code = StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let kind = match e.status {
        401 => "authentication_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 | 503 => "overloaded_error",
        _ => "invalid_request_error",
    };
    let payload = if api == Api::Anthropic {
        json!({"type":"error","error":{"type":kind,"message":e.message}})
    } else {
        json!({"error":{"type":kind,"message":e.message,"code":e.status}})
    };
    (code, axum::Json(payload)).into_response()
}
fn failure(status: u16, msg: impl Into<String>) -> RouteError {
    RouteError::new(status, msg)
}
fn io_stream<S: futures_util::Stream<Item = Result<Bytes, io::Error>>>(stream: S) -> S {
    stream
}
struct BudgetedBytes {
    bytes: Bytes,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl AsRef<[u8]> for BudgetedBytes {
    fn as_ref(&self) -> &[u8] {
        self.bytes.as_ref()
    }
}
const QUANTUM: usize = 64 * 1024;
async fn collect_json(
    body: Body,
    limit: usize,
    idle_ms: Option<u64>,
) -> Result<(Vec<Bytes>, usize), RouteError> {
    let mut input = body.into_data_stream();
    let mut pieces = Vec::new();
    let mut small = BytesMut::new();
    let mut total = 0usize;
    let mut reads = 0usize;
    loop {
        let next = if let Some(ms) = idle_ms {
            tokio::time::timeout(Duration::from_millis(ms), input.next())
                .await
                .map_err(|_| failure(408, "JSON request body idle timeout elapsed"))?
        } else {
            input.next().await
        };
        let Some(chunk) = next else { break };
        let mut chunk = chunk.map_err(|_| failure(400, "request body read failed"))?;
        total = total
            .checked_add(chunk.len())
            .ok_or_else(|| failure(413, "request body length overflow"))?;
        if total > limit {
            return Err(failure(413, "JSON request exceeds byte limit"));
        }
        while !chunk.is_empty() {
            if small.is_empty() && chunk.len() >= QUANTUM {
                pieces.push(chunk.split_to(QUANTUM));
            } else {
                let n = (QUANTUM - small.len()).min(chunk.len());
                small.extend_from_slice(&chunk.split_to(n));
                if small.len() == QUANTUM {
                    pieces.push(small.split().freeze());
                }
            }
        }
        reads += 1;
        if reads.is_multiple_of(4) {
            tokio::task::yield_now().await;
        }
    }
    if !small.is_empty() {
        pieces.push(small.freeze());
    }
    Ok((pieces, total))
}
fn assemble(mut pieces: Vec<Bytes>, total: usize) -> Bytes {
    if pieces.len() == 1 {
        return pieces.pop().unwrap();
    }
    let mut data = BytesMut::with_capacity(total);
    for piece in pieces {
        data.extend_from_slice(&piece);
    }
    data.freeze()
}

fn header(headers: &HeaderMap, name: &str) -> Result<Option<String>, RouteError> {
    let mut values = headers.get_all(name).iter();
    let Some(v) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(failure(400, format!("duplicate {name}")));
    }
    let value = v
        .to_str()
        .map_err(|_| failure(400, format!("invalid {name}")))?;
    valid_id(value).map_err(|e| failure(400, e))?;
    Ok(Some(value.to_string()))
}
fn session(headers: &HeaderMap) -> Result<Option<String>, RouteError> {
    for name in ["x-apibond-session-id", "x-session-id"] {
        if let Some(v) = header(headers, name)? {
            return Ok(Some(v));
        }
    }
    if let Some(root) = header(headers, "x-claude-code-session-id")? {
        // Claude Code shares the root session across independent agent histories.
        // Scope each agent's affinity to that root, never to its parent agent.
        if let Some(agent) = header(headers, "x-claude-code-agent-id")? {
            let key = serde_json::to_vec(&(root, agent)).expect("strings serialize");
            return Ok(Some(format!(
                "claude-agent:{:x}",
                Bytes::copy_from_slice(&Sha256::digest(key))
            )));
        }
        return Ok(Some(root));
    }
    header(headers, "x-codex-session-id")
}
fn json_type(headers: &HeaderMap) -> bool {
    headers
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or("").trim())
        .is_some_and(|s| {
            s.eq_ignore_ascii_case("application/json")
                || (s.starts_with("application/") && s.ends_with("+json"))
        })
}

pub fn clean_headers(source: &HeaderMap, request: bool) -> HeaderMap {
    let nominated = source
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut result = HeaderMap::new();
    for (name, value) in source {
        let n = name.as_str();
        if matches!(
            n,
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "host"
                | "content-length"
        ) || nominated.iter().any(|x| x == n)
        {
            continue;
        }
        if request
            && (matches!(
                n,
                "authorization"
                    | "x-api-key"
                    | "api-key"
                    | "x-session-id"
                    | "x-claude-code-session-id"
                    | "x-codex-session-id"
            ) || n.starts_with("x-apibond-"))
        {
            continue;
        }
        result.append(name.clone(), value.clone());
    }
    result
}

#[derive(Clone, Copy)]
struct Endpoint {
    api: Api,
    json: bool,
    upload: bool,
    observe: bool,
    raw_allowed: bool,
    opaque_url: bool,
}
fn endpoint(method: &Method, path: &str, headers: &HeaderMap) -> Result<Endpoint, RouteError> {
    let post = *method == Method::POST;
    let base = Endpoint {
        api: Api::Openai,
        json: post,
        upload: false,
        observe: false,
        raw_allowed: false,
        opaque_url: false,
    };
    if path == "/v1/chat/completions" && post {
        return Ok(Endpoint {
            raw_allowed: true,
            ..base
        });
    }
    if matches!(path, "/v1/messages" | "/v1/messages/count_tokens") && post {
        return Ok(Endpoint {
            api: Api::Anthropic,
            raw_allowed: path == "/v1/messages",
            ..base
        });
    }
    if path == "/v1/responses" && post {
        return Ok(Endpoint {
            api: Api::Responses,
            observe: true,
            ..base
        });
    }
    if matches!(path, "/v1/responses/compact" | "/v1/responses/input_tokens") && post {
        return Ok(Endpoint {
            api: Api::Responses,
            ..base
        });
    }
    if let Some(rest) = path.strip_prefix("/v1/responses/") {
        let bits = rest.split('/').collect::<Vec<_>>();
        if (bits.len() == 1 && matches!(*method, Method::GET | Method::DELETE))
            || (bits.len() == 2
                && ((bits[1] == "input_items" && *method == Method::GET)
                    || (bits[1] == "cancel" && post)))
        {
            return Ok(Endpoint {
                api: Api::Responses,
                json: false,
                opaque_url: true,
                ..base
            });
        }
    }
    if path == "/v1/files" || path.starts_with("/v1/files/") {
        let explicit = header(headers, "x-apibond-api")?;
        let anth =
            headers.contains_key("anthropic-version") || headers.contains_key("anthropic-beta");
        let api = match (explicit.as_deref(), anth) {
            (Some("openai"), false) => Api::Openai,
            (Some("anthropic"), _) | (None, true) => Api::Anthropic,
            _ => {
                return Err(failure(
                    400,
                    "files require x-apibond-api: openai|anthropic; conflicting native headers are not allowed",
                ));
            }
        };
        let rest = path.strip_prefix("/v1/files/");
        let valid = if let Some(rest) = rest {
            let bits = rest.split('/').collect::<Vec<_>>();
            (bits.len() == 1 && matches!(*method, Method::GET | Method::DELETE))
                || (bits.len() == 2 && bits[1] == "content" && *method == Method::GET)
        } else {
            post || *method == Method::GET
        };
        if valid {
            return Ok(Endpoint {
                api,
                json: false,
                upload: post,
                observe: post,
                raw_allowed: false,
                opaque_url: rest.is_some(),
            });
        }
    }
    Err(failure(404, "unsupported API path or method"))
}

async fn handle(State(app): State<Arc<App>>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let err_api = if path.starts_with("/v1/messages")
        || request
            .headers()
            .get("x-apibond-api")
            .is_some_and(|v| v == "anthropic")
        || request.headers().contains_key("anthropic-version")
    {
        Api::Anthropic
    } else {
        Api::Openai
    };
    if path == "/healthz" && request.method() == Method::GET {
        return axum::Json(json!({"status":"ok","version":env!("CARGO_PKG_VERSION")}))
            .into_response();
    }
    match dispatch(app, request).await {
        Ok(r) => r,
        Err(e) => error(err_api, e),
    }
}

async fn dispatch(app: Arc<App>, request: Request) -> Result<Response, RouteError> {
    let scope = app.authenticate(request.headers())?;
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    if path == "/v1/models" && method == Method::GET {
        return Ok(
            axum::Json(app.models(request.headers().contains_key("anthropic-version")))
                .into_response(),
        );
    }
    if path == "/status" && method == Method::GET {
        return Ok(axum::Json(app.status()).into_response());
    }
    let ep = endpoint(&method, &path, request.headers())?;
    let request_permit = app
        .requests
        .clone()
        .try_acquire_owned()
        .map_err(|_| failure(503, "global request limit reached"))?;
    let raw = match header(request.headers(), "x-apibond-raw")?.as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        _ => return Err(failure(400, "x-apibond-raw must be true or false")),
    };
    if raw && !ep.raw_allowed {
        return Err(failure(
            400,
            "raw mode is only available for Chat/Messages creation",
        ));
    }
    let header_model = header(request.headers(), "x-apibond-model")?;
    let explicit = header(request.headers(), "x-apibond-backend")?;
    let header_session = session(request.headers())?;
    let path_query = request
        .uri()
        .path_and_query()
        .map(|s| s.as_str())
        .unwrap_or(&path)
        .to_string();
    let (parts, body) = request.into_parts();
    let content_length = parts
        .headers
        .get("content-length")
        .map(|v| {
            v.to_str()
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or_else(|| failure(400, "invalid Content-Length"))
        })
        .transpose()?;
    let mut meta = Metadata::default();
    let mut buffered = None;
    let mut body_budget = None;
    let mut original_body = Some(body);
    if ep.json && !raw {
        if !json_type(&parts.headers) {
            return Err(failure(415, "inspected requests require application/json"));
        }
        if parts
            .headers
            .get("content-encoding")
            .is_some_and(|v| v != "identity")
        {
            return Err(failure(
                415,
                "compressed inspected JSON is not supported; send identity encoding",
            ));
        }
        let limit = app.config.limits.max_json_bytes;
        if content_length.is_some_and(|n| n > limit) {
            return Err(failure(413, "JSON request exceeds byte limit"));
        }
        let reserve = content_length
            .unwrap_or(limit)
            .max(1)
            .checked_mul(2)
            .ok_or_else(|| failure(413, "body budget overflow"))?;
        body_budget = Some(
            app.buffer
                .clone()
                .try_acquire_many_owned(reserve as u32)
                .map_err(|_| failure(503, "buffered JSON memory budget exhausted"))?,
        );
        let (pieces, total) = collect_json(
            original_body.take().unwrap(),
            limit,
            app.config.transport.json_body_idle_timeout_ms,
        )
        .await?;
        let retained = (total.max(1) * 2) as u32;
        let budget = body_budget.as_mut().unwrap();
        if budget.num_permits() > retained as usize {
            let release = budget.num_permits() as u32 - retained;
            drop(budget.split(release as usize));
        }
        let fallback = app.config.routing.user_id_fallback;
        let (inspected, data, budget) = if total >= 1024 * 1024 {
            tokio::task::spawn_blocking(move || {
                let data = assemble(pieces, total);
                let result = inspect::inspect(&data, fallback);
                (result, data, body_budget)
            })
            .await
            .map_err(|_| failure(500, "JSON inspection worker failed"))?
        } else {
            let data = assemble(pieces, total);
            (inspect::inspect(&data, fallback), data, body_budget)
        };
        meta = inspected.map_err(|e| failure(400, e))?;
        body_budget = budget;
        if meta.model.is_none() && !(ep.api == Api::Responses && meta.previous.is_some()) {
            return Err(failure(
                400,
                "inspected creation requires a public model in JSON",
            ));
        }
        buffered = Some(data);
    }
    if let (Some(h), Some(b)) = (&header_model, &meta.model)
        && h != b
    {
        return Err(failure(400, "header model conflicts with JSON model"));
    }
    let session_id = header_session.or(meta.session.clone());
    let mut owners = Vec::new();
    let mut unknown = false;
    if let Some(id) = &meta.previous {
        if let Some(owner) = app.owner(scope, ep.api.provider(), id) {
            owners.push(owner);
        } else {
            unknown = true;
        }
    }
    for id in &meta.files {
        if let Some(owner) = app.owner(scope, ep.api.provider(), id) {
            owners.push(owner);
        } else if app.config.routing.strict_file_ownership {
            unknown = true;
        }
    }
    if ep.opaque_url {
        let id = path
            .split('/')
            .nth(3)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| failure(400, "missing resource ID"))?;
        valid_id(id).map_err(|e| failure(400, e))?;
        if !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        {
            return Err(failure(
                400,
                "resource IDs must be ASCII letters, digits, underscore or hyphen",
            ));
        }
        if let Some(owner) = app.owner(scope, ep.api.provider(), id) {
            owners.push(owner);
        } else {
            unknown = true;
        }
    }
    let alias = header_model
        .or(meta.model.clone())
        .or_else(|| owners.first().map(|o| o.alias.clone()))
        .ok_or_else(|| {
            failure(
                400,
                "supply a public model (x-apibond-model for files and unknown resource URLs)",
            )
        })?;
    let pool = app
        .pools
        .get(&alias)
        .ok_or_else(|| failure(404, "unknown public model"))?;
    if raw && !pool.allow_raw {
        return Err(failure(400, "raw mode is not enabled for this model"));
    }
    if raw && !json_type(&parts.headers) {
        return Err(failure(415, "raw mode requires JSON content type"));
    }
    if ep.upload {
        let ct = parts
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.starts_with("multipart/form-data;") || !ct.contains("boundary=") {
            return Err(failure(
                415,
                "file upload requires multipart/form-data with boundary",
            ));
        }
    }
    let selected = app.select(Route {
        alias: &alias,
        api: ep.api,
        scope,
        session: session_id.as_deref(),
        explicit: explicit.as_deref(),
        owners,
        needs_pin: unknown || meta.untracked_state,
    })?;
    if tracing::enabled!(tracing::Level::DEBUG) {
        let session_tag = session_id.as_ref().map(|id| {
            format!(
                "{:x}",
                Bytes::copy_from_slice(&Sha256::digest(id.as_bytes()))
            )
        });
        tracing::debug!(backend=%selected.backend.id, ?session_tag,
            claude_agent=parts.headers.contains_key("x-claude-code-agent-id"),
            "request routed");
    }
    let backend_index = app
        .backends
        .iter()
        .position(|b| Arc::ptr_eq(b, &selected.backend))
        .unwrap();
    let mut upstream_headers = clean_headers(&parts.headers, true);
    selected.backend.auth_headers(ep.api, &mut upstream_headers);
    if ep.observe {
        upstream_headers.insert("accept-encoding", HeaderValue::from_static("identity"));
    }
    let upload_exceeded = Arc::new(AtomicBool::new(false));
    let upstream_body = if let Some(data) = buffered {
        let data = Bytes::from_owner(BudgetedBytes {
            bytes: data,
            _permit: body_budget.take().unwrap(),
        });
        let chunks = inspect::body_segments(data, meta.model_span, &selected.upstream_model);
        let length = chunks.iter().map(Bytes::len).sum::<usize>();
        upstream_headers.insert(
            "content-length",
            HeaderValue::from_str(&length.to_string()).unwrap(),
        );
        reqwest::Body::wrap_stream(io_stream(async_stream::try_stream! {
            let mut count=0usize;
            for mut chunk in chunks {
                while !chunk.is_empty() {
                    let n=QUANTUM.min(chunk.len());yield chunk.split_to(n);
                    count+=1;if count.is_multiple_of(4) {tokio::task::yield_now().await;}
                }
            }
        }))
    } else {
        let limit = if ep.upload {
            app.config.limits.max_upload_bytes
        } else {
            app.config.limits.max_json_bytes
        };
        if content_length.is_some_and(|n| n > limit) {
            return Err(failure(413, "request exceeds streaming byte limit"));
        }
        if let Some(n) = content_length {
            upstream_headers.insert(
                "content-length",
                HeaderValue::from_str(&n.to_string()).unwrap(),
            );
        }
        let mut input = original_body.take().unwrap().into_data_stream();
        let exceeded = upload_exceeded.clone();
        reqwest::Body::wrap_stream(io_stream(async_stream::try_stream! {
            let mut seen=0usize;let mut count=0usize;
            while let Some(chunk)=input.next().await {
                let chunk=chunk.map_err(io::Error::other)?;
                seen=seen.checked_add(chunk.len()).ok_or_else(||io::Error::other("upload size overflow"))?;
                if seen>limit {exceeded.store(true,Ordering::Relaxed);Err(io::Error::other("streaming body byte limit exceeded"))?;}
                let mut chunk=chunk;
                while !chunk.is_empty() {let n=QUANTUM.min(chunk.len());yield chunk.split_to(n);count+=1;if count.is_multiple_of(4) {tokio::task::yield_now().await;}}
            }
        }))
    };
    let send = app
        .client
        .request(method, selected.backend.url(&path_query))
        .headers(upstream_headers)
        .body(upstream_body)
        .send();
    let result = if let Some(ms) = app.config.transport.header_timeout_ms {
        tokio::time::timeout(Duration::from_millis(ms), send)
            .await
            .map_err(|_| failure(504, "configured upstream header timeout elapsed"))?
    } else {
        send.await
    };
    drop(body_budget);
    let upstream = match result {
        Ok(r) => r,
        Err(e) => {
            if upload_exceeded.load(Ordering::Relaxed) {
                return Err(failure(413, "streaming request exceeds byte limit"));
            }
            if e.is_connect() {
                selected.backend.failed_connect();
            }
            return Err(failure(
                502,
                "upstream transport failed (request was not replayed)",
            ));
        }
    };
    if ep.observe
        && upstream.status().is_success()
        && upstream
            .headers()
            .get("content-encoding")
            .is_some_and(|v| v != "identity")
    {
        return Err(failure(
            502,
            "upstream ignored identity encoding on a state-producing response",
        ));
    }
    let status = upstream.status();
    let sse = upstream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.starts_with("text/event-stream"));
    let mut headers = clean_headers(upstream.headers(), false);
    headers.insert(
        "x-apibond-backend",
        HeaderValue::from_str(&selected.backend.id)
            .map_err(|_| failure(500, "invalid configured backend ID header"))?,
    );
    if sse {
        headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    }
    let observe = ep.observe && status.is_success();
    let idle = app.config.transport.body_idle_timeout_ms;
    let mut input = upstream.bytes_stream();
    let mut probe = IdProbe::new(sse);
    let mut observing = observe;
    let output = async_stream::try_stream! {
        let _request_permit=request_permit;let _selection=selected;
        loop {
            let next=if let Some(ms)=idle {
                tokio::time::timeout(Duration::from_millis(ms),input.next()).await.map_err(|_|io::Error::new(io::ErrorKind::TimedOut,"upstream body idle timeout"))?
            } else {input.next().await};
            let Some(chunk)=next else {break;};
            let chunk=chunk.map_err(io::Error::other)?;
            if observing
                && let Some(id)=probe.feed(&chunk) {app.remember(scope,ep.api.provider(),id,&alias,backend_index);observing=false;}
            yield chunk;
        }
    };
    let mut response = Response::new(Body::from_stream(io_stream(output)));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}
