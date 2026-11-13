use apibond::{App, config::Config};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, task::JoinHandle};

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
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server { url, task }
}
async fn upstream(State(calls): State<Arc<AtomicUsize>>, request: Request) -> Response {
    let body = to_bytes(request.into_body(), 2048).await.unwrap();
    let input: Value = serde_json::from_slice(&body).unwrap();
    calls.fetch_add(1, Ordering::Relaxed);
    if let Some(delay) = input["delay_ms"].as_u64() {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    if let Some(delay) = input["stream_gap_ms"].as_u64() {
        let output = async_stream::stream! {
            yield Ok::<_, std::io::Error>(Bytes::from_static(b"data: first\n\n"));
            tokio::time::sleep(Duration::from_millis(delay)).await;
            yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
        };
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(output),
        )
            .into_response();
    }
    axum::Json(json!({ "input": input })).into_response()
}
fn config_value() -> Value {
    serde_json::from_str(include_str!("../apibond.example.json")).unwrap()
}
struct Fixture {
    proxy: Server,
    _upstream: Server,
    app: Arc<App>,
    client: reqwest::Client,
    calls: Arc<AtomicUsize>,
}
impl Fixture {
    async fn new(idle_ms: Option<u64>) -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let upstream = serve(Router::new().fallback(upstream).with_state(calls.clone())).await;
        let mut config = config_value();
        config["limits"]["max_open_requests"] = json!(4);
        config["limits"]["max_json_bytes"] = json!(1024);
        config["limits"]["buffered_json_budget_bytes"] = json!(4096);
        config["transport"]["json_body_idle_timeout_ms"] = json!(idle_ms);
        for backend in config["backends"].as_object_mut().unwrap().values_mut() {
            backend["base_url"] = json!(format!("{}/v1", upstream.url));
        }
        let config: Config = serde_json::from_value(config).unwrap();
        let app = App::new(config).unwrap();
        let proxy = serve(apibond::router(app.clone())).await;
        Self {
            proxy,
            _upstream: upstream,
            app,
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            calls,
        }
    }
    fn post(&self) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}/v1/chat/completions", self.proxy.url))
            .header("content-type", "application/json")
    }
    fn stalled(&self) -> JoinHandle<reqwest::Response> {
        let body = async_stream::stream! {
            yield Ok::<_, std::io::Error>(Bytes::from_static(b"{\"model\":"));
            std::future::pending::<()>().await;
        };
        let request = self.post().body(reqwest::Body::wrap_stream(body));
        tokio::spawn(async move { request.send().await.unwrap() })
    }
    async fn wait_reserved(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.app.buffer.available_permits() != expected {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("JSON memory reservation did not reach expected value");
    }
    async fn wait_idle(&self) {
        self.wait_reserved(self.app.config.limits.buffered_json_budget_bytes)
            .await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.app.requests.available_permits() != self.app.config.limits.max_open_requests
                || self
                    .app
                    .backends
                    .iter()
                    .any(|b| b.active.load(Ordering::Relaxed) != 0)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("request/backend admission did not recover");
    }
}

#[test]
fn json_input_idle_configuration_defaults_null_and_zero() {
    let mut value = config_value();
    value["transport"]
        .as_object_mut()
        .unwrap()
        .remove("json_body_idle_timeout_ms");
    let defaults: Config = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(defaults.transport).unwrap()["json_body_idle_timeout_ms"],
        60_000
    );
    for idle in [json!(null), json!(200), json!(0)] {
        value["transport"]["json_body_idle_timeout_ms"] = idle.clone();
        let config: Config = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(config.validate().is_ok(), idle != 0);
    }
}

#[tokio::test]
async fn stalled_json_uploads_timeout_and_release_all_admission() {
    let f = Fixture::new(Some(400)).await;
    let first = f.stalled();
    let second = f.stalled();
    f.wait_reserved(0).await;
    let rejected = f
        .post()
        .body(r#"{"model":"bonded-model"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 503);
    rejected.bytes().await.unwrap();
    for pending in [first, second] {
        let response = tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .expect("stalled JSON did not return an input timeout")
            .unwrap();
        assert_eq!(response.status(), 408);
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("JSON request body idle")
        );
    }
    f.wait_idle().await;
    assert_eq!(f.calls.load(Ordering::Relaxed), 0);
    let recovered = f
        .post()
        .body(r#"{"model":"bonded-model"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(recovered.status(), 200);
    recovered.bytes().await.unwrap();
    f.wait_idle().await;
    assert_eq!(f.calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn progressing_json_and_long_inference_outlive_input_idle_limit() {
    let f = Fixture::new(Some(300)).await;
    let prompt = json!({
        "model": "bonded-model",
        "messages": [{ "role": "user", "content": "타입스크립트의 특징을 설명하라" }],
    });
    let chunks = serde_json::to_vec(&prompt)
        .unwrap()
        .chunks(12)
        .map(Bytes::copy_from_slice)
        .collect::<Vec<_>>();
    assert!(chunks.len() > 4);
    let body = async_stream::stream! {
        for chunk in chunks {
            tokio::time::sleep(Duration::from_millis(75)).await;
            yield Ok::<_, std::io::Error>(chunk);
        }
    };
    let response = f
        .post()
        .body(reqwest::Body::wrap_stream(body))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let output: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(output["input"]["messages"], prompt["messages"]);
    f.wait_idle().await;
    let response = f
        .post()
        .body(json!({ "model": "bonded-model", "delay_ms": 450, "stream_gap_ms": 450 }).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.text().await.unwrap().contains("[DONE]"));
    f.wait_idle().await;
    assert_eq!(f.calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn null_input_idle_timeout_retains_until_caller_cancels() {
    let f = Fixture::new(None).await;
    let pending = f.stalled();
    f.wait_reserved(2048).await;
    tokio::time::sleep(Duration::from_millis(450)).await;
    assert!(!pending.is_finished());
    assert_eq!(f.app.buffer.available_permits(), 2048);
    pending.abort();
    f.wait_idle().await;
    assert_eq!(f.calls.load(Ordering::Relaxed), 0);
}
