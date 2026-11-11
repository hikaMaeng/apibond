use apibond::{
    App,
    config::{Api, Config},
    routing::Route,
};
use axum::serve::ListenerExt;
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::{StreamExt, future::join_all};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, task::JoinHandle};

#[derive(Clone, Debug)]
struct Seen {
    at: Instant,
    path: String,
    headers: HeaderMap,
    body: Bytes,
}
#[derive(Clone)]
struct Mock {
    id: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    counter: Arc<AtomicUsize>,
}
struct Server {
    url: String,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(router: Router) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener.tap_io(|s| {
                let _ = s.set_nodelay(true);
            }),
            router,
        )
        .await
        .unwrap();
    });
    Server { url, task }
}
async fn mock(State(state): State<Mock>, request: Request) -> Response {
    let path = request.uri().path_and_query().unwrap().as_str().to_string();
    let headers = request.headers().clone();
    let body = to_bytes(request.into_body(), 128 * 1024 * 1024)
        .await
        .unwrap_or_default();
    state.seen.lock().unwrap().push(Seen {
        at: Instant::now(),
        path: path.clone(),
        headers: headers.clone(),
        body: body.clone(),
    });
    if path.starts_with("/v1/files/") {
        return ([("content-type", "application/octet-stream")], "file bytes").into_response();
    }
    if path == "/v1/files" {
        let id = format!(
            "file_{}_{}",
            state.id,
            state.counter.fetch_add(1, Ordering::Relaxed)
        );
        return axum::Json(json!({"id":id,"object":"file"})).into_response();
    }
    let value: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    if value["mode"] == "error" {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "3"), ("x-upstream", "yes")],
            "native error bytes",
        )
            .into_response();
    }
    if value["mode"] == "redirect" {
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", "http://127.0.0.1:1/leak")],
            "redirect",
        )
            .into_response();
    }
    if value["mode"] == "headers_late" {
        tokio::time::sleep(Duration::from_millis(1300)).await;
    }
    let response_id = format!(
        "resp_{}_{}",
        state.id,
        state.counter.fetch_add(1, Ordering::Relaxed)
    );
    if value["mode"] == "gzip" {
        return ([("content-encoding", "gzip")], "compressed garbage").into_response();
    }
    if value["stream"] == true {
        let response = path.starts_with("/v1/responses");
        let payload = if response {
            format!(
                "event: response.created\r\ndata: {{\"response\":{{\"id\":\"{response_id}\"}}}}\r\n\r\n"
            )
        } else {
            "data: {\"text\":\"first\"}\n\n".to_string()
        };
        let output = async_stream::stream! {
            for c in payload.as_bytes().chunks(3) {yield Ok::<_,std::io::Error>(Bytes::copy_from_slice(c));}
            tokio::time::sleep(Duration::from_millis(300)).await;
            yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
        };
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(output),
        )
            .into_response();
    }
    if value["mode"] == "late_id" {
        return (
            [("content-type", "application/json")],
            format!(
                "{{\"output\":\"{}\",\"id\":\"{response_id}\"}}",
                "x".repeat(8 * 1024 * 1024)
            ),
        )
            .into_response();
    }
    if path.starts_with("/v1/responses") {
        return axum::Json(json!({"id":response_id,"object":"response","backend":state.id}))
            .into_response();
    }
    axum::Json(json!({"backend":state.id,"ok":true})).into_response()
}

struct Fixture {
    proxy: Server,
    _a: Server,
    _b: Server,
    app: Arc<App>,
    a: Mock,
    b: Mock,
    client: reqwest::Client,
}
fn config(a: &str, b: &str) -> Config {
    let mut v: Value = serde_json::from_str(include_str!("../apibond.example.json")).unwrap();
    v["backends"]["server-a"]["base_url"] = json!(format!("{a}/v1"));
    v["backends"]["server-b"]["base_url"] = json!(format!("{b}/v1"));
    v["backends"]["server-b"]["apis"] = json!(["openai", "anthropic", "responses"]);
    v["models"]["bonded-model"]["allow_raw"] = json!(true);
    serde_json::from_value(v).unwrap()
}
impl Fixture {
    async fn new() -> Self {
        Self::with(|_| {}).await
    }
    async fn with(change: impl FnOnce(&mut Config)) -> Self {
        let a = Mock {
            id: "a".into(),
            seen: Arc::default(),
            counter: Arc::default(),
        };
        let b = Mock {
            id: "b".into(),
            seen: Arc::default(),
            counter: Arc::default(),
        };
        let sa = serve(Router::new().fallback(mock).with_state(a.clone())).await;
        let sb = serve(Router::new().fallback(mock).with_state(b.clone())).await;
        let mut cfg = config(&sa.url, &sb.url);
        change(&mut cfg);
        let app = App::new(cfg).unwrap();
        let proxy = serve(apibond::router(app.clone())).await;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Self {
            proxy,
            _a: sa,
            _b: sb,
            app,
            a,
            b,
            client,
        }
    }
    fn post(&self, path: &str, value: Value) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.proxy.url))
            .header("content-type", "application/json")
            .body(value.to_string())
    }
    fn seen(&self) -> Vec<Seen> {
        let mut result = self.a.seen.lock().unwrap().clone();
        result.extend(self.b.seen.lock().unwrap().clone());
        result.sort_by_key(|s| s.at);
        result
    }
    async fn wait_idle(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if self
                    .app
                    .backends
                    .iter()
                    .all(|b| b.active.load(Ordering::Relaxed) == 0)
                    && self.app.requests.available_permits()
                        == self.app.config.limits.max_open_requests
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("capacity leaked");
    }
}

#[tokio::test]
async fn native_protocols_and_multimodal_bytes() {
    let f = Fixture::new().await;
    for path in [
        "/v1/chat/completions",
        "/v1/messages",
        "/v1/messages/count_tokens",
        "/v1/responses",
        "/v1/responses/compact",
        "/v1/responses/input_tokens",
    ] {
        let raw = format!(
            r#"{{ "model" : "bonded-model", "messages":[{{"role":"user","content":[{{"type":"image","source":{{"type":"base64","data":"{}"}}}},{{"type":"text","text":"한글 😀 \\ \" escaped"}}]}}],"tools":[{{"type":"function","function":{{"name":"f","parameters":{{"example":{{"file_id":"ignore"}}}}}}}}],"metadata":{{"file_id":"ignore"}},"unknown":1.2300 }}"#,
            "A".repeat(1024 * 1024)
        );
        let result = f
            .client
            .post(format!("{}{path}?native=1", f.proxy.url))
            .header("content-type", "application/json; charset=utf-8")
            .body(raw.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(result.status(), 200, "{}", result.text().await.unwrap());
        let seen = f.seen().pop().unwrap();
        assert_eq!(seen.path, format!("{path}?native=1"));
        assert_eq!(
            seen.body.as_ref(),
            raw.replace("\"bonded-model\"", "\"actual-model\"")
                .as_bytes()
        );
    }
}

#[tokio::test]
async fn concurrent_first_session_is_atomic_and_new_sessions_balance() {
    let f = Fixture::new().await;
    let calls = (0..40).map(|_| {
        f.post("/v1/chat/completions", json!({"model":"bonded-model"}))
            .header("x-session-id", "same")
            .send()
    });
    let replies = join_all(calls).await;
    let mut owner = None;
    for response in replies {
        let response = response.unwrap();
        assert_eq!(response.status(), 200);
        let id = response.headers()["x-apibond-backend"]
            .to_str()
            .unwrap()
            .to_string();
        assert!(owner.as_ref().is_none_or(|o| o == &id));
        owner = Some(id);
        response.bytes().await.unwrap();
    }
    let f = Fixture::new().await;
    for i in 0..10 {
        f.post("/v1/chat/completions", json!({"model":"bonded-model"}))
            .header("x-session-id", format!("s{i}"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
    }
    assert_eq!(f.a.seen.lock().unwrap().len(), 5);
    assert_eq!(f.b.seen.lock().unwrap().len(), 5);
    f.wait_idle().await;
}

#[tokio::test]
async fn claude_agents_balance_and_keep_their_own_affinity() {
    let f = Fixture::new().await;
    let mut owners = Vec::new();
    // Real Claude Code header shape: one root session, distinct agent histories.
    for root in ["root-one", "root-two"] {
        for agent in ["main", "worker-a", "worker-b", "worker-c"] {
            let mut owner = None;
            for _ in 0..2 {
                let response = f
                    .post("/v1/messages", json!({"model":"bonded-model"}))
                    .header("x-claude-code-session-id", root)
                    .header("x-claude-code-agent-id", agent)
                    .header("x-claude-code-parent-agent-id", "main")
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 200);
                let backend = response.headers()["x-apibond-backend"]
                    .to_str()
                    .unwrap()
                    .to_owned();
                assert!(owner.as_ref().is_none_or(|o| o == &backend));
                owner = Some(backend);
                response.bytes().await.unwrap();
            }
            owners.push(owner.unwrap());
        }
    }
    assert_eq!(owners.iter().filter(|o| *o == "server-a").count(), 4);
    assert_eq!(owners.iter().filter(|o| *o == "server-b").count(), 4);
    assert_eq!(f.app.status()["models"]["bonded-model"], 8);
    // An explicit APIBond session remains authoritative, even across agents.
    let mut owner = None;
    for agent in ["main", "worker-a"] {
        let response = f
            .post("/v1/messages", json!({"model":"bonded-model"}))
            .header("x-apibond-session-id", "explicit")
            .header("x-claude-code-session-id", "root-one")
            .header("x-claude-code-agent-id", agent)
            .send()
            .await
            .unwrap();
        let backend = response.headers()["x-apibond-backend"]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(owner.as_ref().is_none_or(|o| o == &backend));
        owner = Some(backend);
        response.bytes().await.unwrap();
    }
    f.wait_idle().await;
}

#[tokio::test]
async fn concurrent_claude_agents_use_both_backends_without_moving_an_agent() {
    let f = Fixture::with(|c| {
        for b in c.backends.values_mut() {
            b.capacity = 4;
        }
    })
    .await;
    let replies = join_all((0..8).map(|i| {
        f.post(
            "/v1/messages",
            json!({"model":"bonded-model","stream":true}),
        )
        .header("x-claude-code-session-id", "shared-root")
        .header("x-claude-code-agent-id", format!("agent-{i}"))
        .send()
    }))
    .await;
    let mut owners = Vec::new();
    let mut streams = Vec::new();
    for response in replies {
        let response = response.unwrap();
        assert_eq!(response.status(), 200);
        owners.push(
            response.headers()["x-apibond-backend"]
                .to_str()
                .unwrap()
                .to_owned(),
        );
        streams.push(response.bytes_stream());
    }
    assert_eq!(owners.iter().filter(|o| *o == "server-a").count(), 4);
    assert_eq!(owners.iter().filter(|o| *o == "server-b").count(), 4);
    for stream in &mut streams {
        while stream.next().await.is_some() {}
    }
    f.wait_idle().await;
    for (i, owner) in owners.iter().enumerate() {
        let response = f
            .post("/v1/messages", json!({"model":"bonded-model"}))
            .header("x-claude-code-session-id", "shared-root")
            .header("x-claude-code-agent-id", format!("agent-{i}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.headers()["x-apibond-backend"], owner.as_str());
        response.bytes().await.unwrap();
    }
    f.wait_idle().await;
}

#[tokio::test]
async fn streaming_holds_capacity_and_cancels() {
    let f = Fixture::with(|c| {
        c.backends.get_mut("server-a").unwrap().capacity = 1;
        c.backends.get_mut("server-b").unwrap().capacity = 1;
    })
    .await;
    let start = Instant::now();
    let response = f
        .post(
            "/v1/chat/completions",
            json!({"model":"bonded-model","stream":true}),
        )
        .header("x-session-id", "s")
        .send()
        .await
        .unwrap();
    let backend = response.headers()["x-apibond-backend"]
        .to_str()
        .unwrap()
        .to_string();
    let mut body = response.bytes_stream();
    assert!(body.next().await.unwrap().unwrap().starts_with(b"dat"));
    assert!(start.elapsed() < Duration::from_millis(250));
    assert_eq!(
        f.app
            .backends
            .iter()
            .find(|b| b.id == backend)
            .unwrap()
            .active
            .load(Ordering::Relaxed),
        1
    );
    let rejected = f
        .post("/v1/chat/completions", json!({"model":"bonded-model"}))
        .header("x-session-id", "s")
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 503);
    drop(body);
    f.wait_idle().await;
}

#[tokio::test]
async fn response_ids_native_lifecycle_and_unknown_state() {
    let f = Fixture::new().await;
    for streaming in [false, true] {
        let response = f
            .post(
                "/v1/responses",
                json!({"model":"bonded-model","stream":streaming}),
            )
            .header("accept-encoding", "gzip")
            .send()
            .await
            .unwrap();
        let backend = response.headers()["x-apibond-backend"]
            .to_str()
            .unwrap()
            .to_string();
        let raw = response.text().await.unwrap();
        let id = if streaming {
            raw.split("\"id\":\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .to_string()
        } else {
            serde_json::from_str::<Value>(&raw).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let next = f
            .post(
                "/v1/responses",
                json!({"model":"bonded-model","previous_response_id":id}),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(next.status(), 200);
        assert_eq!(next.headers()["x-apibond-backend"], backend);
        next.bytes().await.unwrap();
        for (method, suffix) in [
            ("GET", ""),
            ("DELETE", ""),
            ("GET", "/input_items"),
            ("POST", "/cancel"),
        ] {
            let result = f
                .client
                .request(
                    method.parse().unwrap(),
                    format!("{}/v1/responses/{id}{suffix}", f.proxy.url),
                )
                .send()
                .await
                .unwrap();
            assert_eq!(result.status(), 200);
            assert_eq!(result.headers()["x-apibond-backend"], backend);
            result.bytes().await.unwrap();
        }
    }
    assert!(
        f.seen()
            .iter()
            .filter(|s| s.path == "/v1/responses")
            .all(|s| s.headers["accept-encoding"] == "identity")
    );
    let count = f.seen().len();
    let unknown = f
        .post(
            "/v1/responses",
            json!({"model":"bonded-model","previous_response_id":"unknown"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 409);
    assert_eq!(f.seen().len(), count);
}

#[tokio::test]
async fn late_response_id_and_unexpected_encoding() {
    let f = Fixture::new().await;
    let res = f
        .post(
            "/v1/responses",
            json!({"model":"bonded-model","mode":"late_id"}),
        )
        .send()
        .await
        .unwrap();
    let backend = res.headers()["x-apibond-backend"].clone();
    let bytes = res.bytes().await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let next = f
        .post(
            "/v1/responses",
            json!({"model":"bonded-model","previous_response_id":v["id"]}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(next.status(), 200);
    assert_eq!(next.headers()["x-apibond-backend"], backend);
    next.bytes().await.unwrap();
    let res = f
        .post(
            "/v1/responses",
            json!({"model":"bonded-model","mode":"gzip"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 502);
    assert!(!res.headers().contains_key("content-encoding"));
}

#[tokio::test]
async fn files_upload_reference_and_content() {
    let f = Fixture::new().await;
    for family in ["openai", "anthropic"] {
        let multipart = "--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.7 native bytes\r\n--test--\r\n";
        let res = f
            .client
            .post(format!("{}/v1/files", f.proxy.url))
            .header("x-apibond-model", "bonded-model")
            .header("x-apibond-api", family)
            .header("content-type", "multipart/form-data; boundary=test")
            .body(multipart)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let backend = res.headers()["x-apibond-backend"].clone();
        let id: Value = serde_json::from_str(&res.text().await.unwrap()).unwrap();
        let bodies = if family == "openai" {
            vec![
                json!({"type":"input_file","file_id":id["id"]}),
                json!({"type":"input_image","file_id":id["id"]}),
            ]
        } else {
            vec![
                json!({"type":"container_upload","file_id":id["id"]}),
                json!({"type":"document","source":{"type":"file","file_id":id["id"]}}),
            ]
        };
        for content in bodies {
            let (path, body) = if family == "openai" {
                (
                    "/v1/responses",
                    json!({"model":"bonded-model","input":[{"role":"user","content":[content]}]}),
                )
            } else {
                (
                    "/v1/messages",
                    json!({"model":"bonded-model","messages":[{"role":"user","content":[content]}]}),
                )
            };
            let res = f.post(path, body).send().await.unwrap();
            assert_eq!(res.status(), 200);
            assert_eq!(res.headers()["x-apibond-backend"], backend);
            res.bytes().await.unwrap();
        }
        let res = f
            .client
            .get(format!(
                "{}/v1/files/{}/content?download=1",
                f.proxy.url,
                id["id"].as_str().unwrap()
            ))
            .header("x-apibond-api", family)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(res.text().await.unwrap(), "file bytes");
        assert!(
            f.seen()
                .iter()
                .any(|s| s.path == "/v1/files" && s.body == multipart)
        );
    }
}

#[tokio::test]
async fn state_conflicts_and_untracked_handles_require_existing_pin() {
    let f = Fixture::new().await;
    let res = f
        .post("/v1/responses", json!({"model":"bonded-model"}))
        .header("x-apibond-backend", "server-a")
        .send()
        .await
        .unwrap();
    let id: Value = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    let res = f
        .post(
            "/v1/responses",
            json!({"model":"bonded-model","previous_response_id":id["id"]}),
        )
        .header("x-apibond-backend", "server-b")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    for mut body in [
        json!({"conversation":"conv_unknown"}),
        json!({"input":[{"type":"item_reference","id":"msg_unknown"}]}),
        json!({"input":[{"type":"input_image","file_id":"file_unknown"}]}),
    ] {
        body["model"] = json!("bonded-model");
        let count = f.seen().len();
        let res = f.post("/v1/responses", body.clone()).send().await.unwrap();
        assert_eq!(res.status(), 409);
        assert_eq!(f.seen().len(), count);
        let res = f
            .post("/v1/responses", body)
            .header("x-apibond-backend", "server-a")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        res.bytes().await.unwrap();
    }
    let f = Fixture::with(|c| c.routing.max_state_entries = 1).await;
    let res = f
        .post("/v1/responses", json!({"model":"bonded-model"}))
        .header("x-session-id", "stable")
        .send()
        .await
        .unwrap();
    let old: Value = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    f.post("/v1/responses", json!({"model":"bonded-model"}))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let res = f
        .post(
            "/v1/responses",
            json!({"model":"bonded-model","previous_response_id":old["id"]}),
        )
        .header("x-session-id", "stable")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.bytes().await.unwrap();
}

#[tokio::test]
async fn raw_opt_in_and_native_errors_and_headers() {
    let f = Fixture::new().await;
    let raw = r#"{ "model":"actual-model", "unknown":{"huge":"data"} }"#;
    let res = f
        .client
        .post(format!("{}/v1/chat/completions", f.proxy.url))
        .header("content-type", "application/json")
        .header("x-apibond-model", "bonded-model")
        .header("x-apibond-raw", "true")
        .header("authorization", "Bearer router-secret")
        .header("x-api-key", "second-key")
        .body(raw)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let res = f
        .client
        .post(format!("{}/v1/chat/completions", f.proxy.url))
        .header("content-type", "application/json")
        .header("x-apibond-model", "bonded-model")
        .header("x-apibond-raw", "true")
        .header("authorization", "Bearer router-secret")
        .header("connection", "x-secret-hop")
        .header("x-secret-hop", "remove")
        .body(raw)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.bytes().await.unwrap();
    let seen = f.seen().pop().unwrap();
    assert_eq!(seen.body, raw);
    for key in [
        "authorization",
        "x-api-key",
        "x-apibond-model",
        "x-apibond-raw",
        "x-secret-hop",
    ] {
        assert!(!seen.headers.contains_key(key));
    }
    for path in ["/v1/responses", "/v1/messages/count_tokens", "/v1/files"] {
        let res = f
            .post(path, json!({"model":"bonded-model"}))
            .header("x-apibond-raw", "true")
            .header("x-apibond-api", "openai")
            .send()
            .await
            .unwrap();
        assert!(res.status().is_client_error());
    }
    let res = f
        .post(
            "/v1/chat/completions",
            json!({"model":"bonded-model","mode":"error"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 429);
    assert_eq!(res.headers()["retry-after"], "3");
    assert_eq!(res.text().await.unwrap(), "native error bytes");
    let res = f
        .post(
            "/v1/chat/completions",
            json!({"model":"bonded-model","mode":"redirect"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 307);
}

#[tokio::test]
async fn body_limits_and_duplicate_keys_and_type_errors() {
    let f = Fixture::with(|c| {
        c.limits.max_json_bytes = 1024;
        c.limits.buffered_json_budget_bytes = 4096;
    })
    .await;
    for (body, status, encoding) in [
        (
            r#"{"model":"bonded-model","mod\u0065l":"other"}"#.to_string(),
            400,
            None,
        ),
        ("{".to_string(), 400, None),
        (
            format!(
                "{{\"model\":\"bonded-model\",\"data\":\"{}\"}}",
                "A".repeat(2048)
            ),
            413,
            None,
        ),
        ("{}".to_string(), 415, Some("gzip")),
    ] {
        let mut req = f
            .client
            .post(format!("{}/v1/chat/completions", f.proxy.url))
            .header("content-type", "application/json")
            .body(body);
        if let Some(e) = encoding {
            req = req.header("content-encoding", e);
        }
        assert_eq!(req.send().await.unwrap().status().as_u16(), status);
    }
    assert!(f.seen().is_empty());
    let chunks = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(format!(
        "{{\"model\":\"bonded-model\",\"data\":\"{}\"}}",
        "A".repeat(2048)
    )))]);
    let res = f
        .client
        .post(format!("{}/v1/chat/completions", f.proxy.url))
        .header("content-type", "application/json")
        .body(reqwest::Body::wrap_stream(chunks))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 413);
}

#[tokio::test]
async fn default_no_header_timeout_and_family_conflict() {
    let f = Fixture::with(|c| {
        c.transport.connect_timeout_ms = 1000;
        c.backends.get_mut("server-a").unwrap().apis = vec![Api::Openai];
    })
    .await;
    let res = f
        .post(
            "/v1/chat/completions",
            json!({"model":"bonded-model","mode":"headers_late"}),
        )
        .header("x-session-id", "family")
        .header("x-apibond-backend", "server-a")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.bytes().await.unwrap();
    assert!(f.app.backends[0].available());
    let res = f
        .post("/v1/messages", json!({"model":"bonded-model"}))
        .header("x-session-id", "family")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let res = f
        .client
        .get(format!("{}/v1/files", f.proxy.url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let res = f
        .client
        .get(format!("{}/v1/files", f.proxy.url))
        .header("x-apibond-api", "openai")
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

#[test]
fn shared_backend_capacity_and_map_lru() {
    let mut cfg = config("http://127.0.0.1:1", "http://127.0.0.1:2");
    cfg.limits.max_open_requests = 1;
    cfg.routing.max_sessions_per_model = 2;
    cfg.backends.get_mut("server-a").unwrap().capacity = 1;
    cfg.models
        .get_mut("bonded-model")
        .unwrap()
        .backends
        .truncate(1);
    cfg.models
        .insert("second-model".into(), cfg.models["bonded-model"].clone());
    let app = App::new(cfg).unwrap();
    let select = |alias: &str, session: &str| {
        app.select(Route {
            alias,
            api: Api::Openai,
            scope: [0; 32],
            session: Some(session),
            explicit: None,
            owners: vec![],
            needs_pin: false,
        })
    };
    let first = select("bonded-model", "a").unwrap();
    assert_eq!(select("second-model", "b").err().unwrap().status, 503);
    drop(first);
    drop(select("bonded-model", "b").unwrap());
    drop(select("bonded-model", "c").unwrap());
    assert_eq!(app.status()["models"]["bonded-model"], 2);
}

#[test]
fn configuration_failures_are_explicit() {
    let original: Value = serde_json::from_str(include_str!("../apibond.example.json")).unwrap();
    let mut bad = original.clone();
    bad.as_object_mut().unwrap().remove("inbound_auth");
    assert!(serde_json::from_value::<Config>(bad).is_err());
    let mut cfg: Config = serde_json::from_value(original).unwrap();
    cfg.models.get_mut("bonded-model").unwrap().allow_raw = true;
    cfg.models.get_mut("bonded-model").unwrap().backends[1].model = "different".into();
    assert!(cfg.validate().is_err());
    cfg.models.get_mut("bonded-model").unwrap().allow_raw = false;
    cfg.backends
        .get_mut("server-a")
        .unwrap()
        .headers
        .insert("X-Api-Key".into(), "leak".into());
    assert!(cfg.validate().is_err());
}

#[tokio::test]
async fn chunked_memory_admission_never_waits_with_partial_reservations() {
    let f = Fixture::with(|c| {
        c.limits.max_json_bytes = 1024;
        c.limits.buffered_json_budget_bytes = 2048;
    })
    .await;
    let stalled = async_stream::stream! {
        yield Ok::<_,std::io::Error>(Bytes::from_static(b"{\"model\":"));
        std::future::pending::<()>().await;
    };
    let req = f
        .client
        .post(format!("{}/v1/chat/completions", f.proxy.url))
        .header("content-type", "application/json")
        .body(reqwest::Body::wrap_stream(stalled));
    let task = tokio::spawn(async move { req.send().await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.app.buffer.available_permits() != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let response = f
        .post("/v1/chat/completions", json!({"model":"bonded-model"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    task.abort();
    f.wait_idle().await;
    assert_eq!(f.app.buffer.available_permits(), 2048);
}

#[tokio::test]
async fn session_connect_failure_does_not_replay_and_next_stateless_request_can_move() {
    let f = Fixture::new().await;
    let response = f
        .post("/v1/chat/completions", json!({"model":"bonded-model"}))
        .header("x-session-id", "s")
        .header("x-apibond-backend", "server-a")
        .send()
        .await
        .unwrap();
    response.bytes().await.unwrap();
    f._a.task.abort();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let response = f
        .post("/v1/chat/completions", json!({"model":"bonded-model"}))
        .header("x-session-id", "s")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    assert!(f.b.seen.lock().unwrap().is_empty());
    let response = f
        .post("/v1/chat/completions", json!({"model":"bonded-model"}))
        .header("x-session-id", "s")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-apibond-backend"], "server-b");
    response.bytes().await.unwrap();
}

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn actual_binary_authentication_scope_and_upstream_credential_replacement() {
    use apibond::config::{AuthScheme, BackendAuth, InboundAuth};
    let f = Fixture::new().await;
    let mut config = f.app.config.clone();
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    config.listen = reservation.local_addr().unwrap();
    drop(reservation);
    config.inbound_auth = InboundAuth::Keys {
        key_envs: vec!["APIBOND_TEST_FRONT".into(), "APIBOND_TEST_OTHER".into()],
    };
    for b in config.backends.values_mut() {
        b.auth = BackendAuth {
            scheme: AuthScheme::Auto,
            env: Some("APIBOND_TEST_BACK".into()),
        };
    }
    let path = std::env::temp_dir().join(format!(
        "apibond-auth-{}-{}.json",
        std::process::id(),
        config.listen.port()
    ));
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_apibond"))
        .arg("--config")
        .arg(&path)
        .env("APIBOND_TEST_FRONT", "front-key")
        .env("APIBOND_TEST_OTHER", "other-key")
        .env("APIBOND_TEST_BACK", "backend-key")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _child = ChildGuard(child);
    let base = format!("http://{}", config.listen);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if f.client.get(format!("{base}/healthz")).send().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let make = |path: &str| {
        f.client
            .post(format!("{base}{path}"))
            .header("content-type", "application/json")
            .body(r#"{"model":"bonded-model"}"#)
    };
    assert_eq!(make("/v1/messages").send().await.unwrap().status(), 401);
    let response = make("/v1/messages")
        .header("x-api-key", "front-key")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.bytes().await.unwrap();
    let seen = f.seen().pop().unwrap();
    assert_eq!(seen.headers["x-api-key"], "backend-key");
    assert!(!seen.headers.contains_key("authorization"));
    let response = make("/v1/responses")
        .header("authorization", "Bearer front-key")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let id: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    let seen = f.seen().pop().unwrap();
    assert_eq!(seen.headers["authorization"], "Bearer backend-key");
    assert!(!seen.headers.contains_key("x-api-key"));
    let response = f
        .client
        .post(format!("{base}/v1/responses"))
        .header("authorization", "Bearer other-key")
        .header("content-type", "application/json")
        .body(json!({"model":"bonded-model","previous_response_id":id["id"]}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn review_tool_arguments_and_null_fields_remain_opaque() {
    let f = Fixture::new().await;
    let long = "x".repeat(600);
    for input in [
        json!({"type":7}),
        json!({"source":{"type":"file","file_id":"untracked-tool-argument"}}),
        json!({long:42}),
    ] {
        let body = json!({"model":"bonded-model","messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t","name":"x","input":input}]}]});
        let original = body.to_string();
        let response = f.post("/v1/messages", body).send().await.unwrap();
        assert_eq!(response.status(), 200);
        response.bytes().await.unwrap();
        assert_eq!(
            f.seen().pop().unwrap().body.as_ref(),
            original.replace("bonded-model", "actual-model").as_bytes()
        );
    }
    for body in [
        json!({"model":"bonded-model","messages":[],"tools":null}),
        json!({"model":"bonded-model","tools":[{"type":"code_interpreter","container":{"type":"auto","file_ids":null}}]}),
    ] {
        let response = f.post("/v1/chat/completions", body).send().await.unwrap();
        assert_eq!(response.status(), 200);
        response.bytes().await.unwrap();
    }
}

#[tokio::test]
async fn review_small_chunked_requests_release_upload_budget_before_inference_finishes() {
    let f = Fixture::with(|c| {
        c.limits.max_json_bytes = 1024;
        c.limits.buffered_json_budget_bytes = 4096;
    })
    .await;
    let mut tasks = Vec::new();
    for _ in 0..3 {
        let input = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from_static(
            br#"{"model":"bonded-model","mode":"headers_late"}"#,
        ))]);
        let req = f
            .client
            .post(format!("{}/v1/chat/completions", f.proxy.url))
            .header("content-type", "application/json")
            .body(reqwest::Body::wrap_stream(input));
        tasks.push(tokio::spawn(async move {
            let r = req.send().await.unwrap();
            let status = r.status();
            r.bytes().await.unwrap();
            status
        }));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            f.app.buffer.available_permits(),
            4096,
            "uploaded JSON still owns admission bytes during inference"
        );
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), 200);
    }
    f.wait_idle().await;
}

#[tokio::test]
async fn review_resource_paths_cannot_normalize_outside_the_api() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let f = Fixture::new().await;
    let addr = f.proxy.url.strip_prefix("http://").unwrap();
    for path in [
        "/v1/responses/..",
        "/v1/responses/%2e%2e",
        "/v1/files/%2e%2e/content",
        "/v1/files/a%2fb/content",
        "/v1/responses/a%5cb",
    ] {
        let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {addr}\r\nx-apibond-model: bonded-model\r\nx-apibond-backend: server-a\r\nx-apibond-api: openai\r\nConnection: close\r\n\r\n"
        );
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut output = Vec::new();
        socket.read_to_end(&mut output).await.unwrap();
        assert!(
            output.starts_with(b"HTTP/1.1 400"),
            "{}",
            String::from_utf8_lossy(&output)
        );
    }
    assert!(f.seen().is_empty());
    let r = f
        .client
        .post(format!("{}/v1/chat/completions", f.proxy.url))
        .header("content-type", "application/json")
        .header("x-apibond-model", "bonded-model")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}

#[test]
fn review_state_lru_is_bounded_and_collisions_need_a_pin() {
    let app = App::new(config("http://127.0.0.1:1", "http://127.0.0.1:2")).unwrap();
    for i in 0..110000 {
        app.remember([0; 32], "openai", format!("resp_{i}"), "bonded-model", 0);
    }
    assert_eq!(app.status()["state_entries"], 100000);
    app.remember(
        [0; 32],
        "openai",
        "resp_collision".into(),
        "bonded-model",
        0,
    );
    app.remember(
        [0; 32],
        "openai",
        "resp_collision".into(),
        "bonded-model",
        1,
    );
    assert!(app.owner([0; 32], "openai", "resp_collision").is_none());
}

#[test]
fn review_sorting_snapshots_remain_ordered_with_live_counter_changes() {
    let mut cfg = config("http://127.0.0.1:1", "http://127.0.0.1:2");
    let template = cfg.backends["server-a"].clone();
    cfg.backends.clear();
    let mut bindings = Vec::new();
    for i in 0..32 {
        let id = format!("b{i:02}");
        cfg.backends.insert(id.clone(), template.clone());
        bindings.push(apibond::config::Binding {
            backend: id,
            model: "actual-model".into(),
        });
    }
    cfg.models.get_mut("bonded-model").unwrap().backends = bindings;
    let app = App::new(cfg).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let changing = app.clone();
    let done = stop.clone();
    let worker = std::thread::spawn(move || {
        let mut count = 0;
        while !done.load(Ordering::Relaxed) {
            for b in &changing.backends {
                b.queue.store(count % 11, Ordering::Relaxed);
                count += 1;
            }
        }
    });
    for _ in 0..100000 {
        drop(
            app.select(Route {
                alias: "bonded-model",
                api: Api::Openai,
                scope: [0; 32],
                session: None,
                explicit: None,
                owners: vec![],
                needs_pin: false,
            })
            .unwrap(),
        );
    }
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
}
