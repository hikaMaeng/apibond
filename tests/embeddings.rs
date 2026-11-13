use apibond::{App, config::Config};
use axum::{
    Router,
    body::{Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
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
#[derive(Clone)]
struct Seen {
    path: String,
    headers: HeaderMap,
    body: Bytes,
}
type Calls = Arc<Mutex<Vec<Seen>>>;
async fn upstream(State(calls): State<Calls>, request: Request) -> Response {
    let path = request.uri().path_and_query().unwrap().as_str().to_string();
    let headers = request.headers().clone();
    let body = to_bytes(request.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    calls.lock().unwrap().push(Seen {
        path,
        headers,
        body: body.clone(),
    });
    let input: Value = serde_json::from_slice(&body).unwrap();
    if input["input"] == json!([]) {
        return (
            StatusCode::BAD_REQUEST,
            [
                ("content-type", "application/json"),
                ("x-native", "empty-batch"),
            ],
            "{\"error\":{\"message\":\"input cannot be empty\",\"type\":\"native_error\"}}",
        )
            .into_response();
    }
    if input["input"] == "native-rate-limit" {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("content-type", "application/json"), ("retry-after", "7")],
            "{ \"error\": {\"message\":\"native limit\"} }",
        )
            .into_response();
    }
    let count = match input["input"].as_array() {
        Some(items) if items.first().is_some_and(|v| v.is_string() || v.is_array()) => items.len(),
        _ => 1,
    };
    let data = (0..count)
        .map(|index| {
            let embedding = if input["encoding_format"] == "base64" {
                json!("AACAPwAAAEA=")
            } else {
                json!([index as f64 + 0.25, -0.5])
            };
            json!({"object":"embedding","index":index,"embedding":embedding})
        })
        .collect::<Vec<_>>();
    (
        [("content-type", "application/json"), ("x-native", "embedding")],
        format!(
            " {{\"object\":\"list\",\"model\":\"upstream-embed\",\"data\":{},\"usage\":{{\"prompt_tokens\":17,\"total_tokens\":17}}}}\n",
            serde_json::to_string(&data).unwrap()
        ),
    )
        .into_response()
}
struct Fixture {
    proxy: Server,
    _upstream: Server,
    app: Arc<App>,
    calls: Calls,
    client: reqwest::Client,
}
impl Fixture {
    async fn new(api: &str) -> Self {
        let calls = Calls::default();
        let backend = serve(Router::new().fallback(upstream).with_state(calls.clone())).await;
        let config: Config = serde_json::from_value(json!({
            "schema_version":1,"listen":"127.0.0.1:0","inbound_auth":{"mode":"none"},
            "backends":{"node":{"base_url":format!("{}/v1",backend.url),"apis":[api],"auth":{"scheme":"none"}}},
            "models":{"bonded-embedding":{"backends":[{"backend":"node","model":"upstream-embed"}]}}
        }))
        .unwrap();
        let app = App::new(config).unwrap();
        let proxy = serve(apibond::router(app.clone())).await;
        Self {
            proxy,
            _upstream: backend,
            app,
            calls,
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }
    fn post(&self, path: &str, body: impl Into<reqwest::Body>) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.proxy.url))
            .header("content-type", "application/json")
            .body(body)
    }
    async fn assert_idle(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if self.app.status()["backends"][0]["active"] == 0
                    && self.app.status()["buffer_bytes_reserved"] == 0
                    && self.app.requests.available_permits()
                        == self.app.config.limits.max_open_requests
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn embeddings_endpoint_requires_its_own_capability() {
    let chat = Fixture::new("openai").await;
    let denied = chat
        .post(
            "/v1/embeddings",
            r#"{"model":"bonded-embedding","input":"text"}"#,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 503);
    assert!(chat.calls.lock().unwrap().is_empty());
    let embedding = Fixture::new("embeddings").await;
    for path in ["/v1/chat/completions", "/v1/messages", "/v1/responses"] {
        let denied = embedding
            .post(path, r#"{"model":"bonded-embedding","input":"text"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), 503, "{path}");
    }
    assert!(embedding.calls.lock().unwrap().is_empty());
    let models = embedding
        .client
        .get(format!("{}/v1/models", embedding.proxy.url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let models: Value = serde_json::from_str(&models).unwrap();
    assert_eq!(models["data"][0]["apibond"]["apis"], json!(["embeddings"]));
    assert_eq!(
        embedding
            .client
            .get(format!("{}/v1/embeddings", embedding.proxy.url))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    embedding.assert_idle().await;
}

#[tokio::test]
async fn embeddings_preserve_single_and_batch_inputs_without_splitting() {
    let f = Fixture::new("embeddings").await;
    for (input, expected) in [
        (json!("타입스크립트의 특징을 설명하라"), 1),
        (json!(["타입스크립트", "러스트", "파이썬"]), 3),
        (json!([11, 22, 33]), 1),
        (json!([[11, 22], [33, 44]]), 2),
    ] {
        let before = f.calls.lock().unwrap().len();
        let original = format!(
            "{{ \"model\" : \"bonded-embedding\", \"input\" : {}, \"dimensions\":2, \"encoding_format\":\"float\", \"user\":\"end-user\", \"unknown\":1.2300 }}",
            serde_json::to_string(&input).unwrap()
        );
        let response = f
            .post("/v1/embeddings?trace=kept", original.clone())
            .header("authorization", "Bearer caller-secret")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["x-apibond-backend"], "node");
        assert_eq!(response.headers()["x-native"], "embedding");
        let raw = response.text().await.unwrap();
        assert!(raw.starts_with(" {\"object\":\"list\""));
        assert!(raw.ends_with('\n'));
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["data"].as_array().unwrap().len(), expected);
        assert_eq!(
            value["usage"],
            json!({"prompt_tokens":17,"total_tokens":17})
        );
        for index in 0..expected {
            assert_eq!(value["data"][index]["index"], index);
            assert_eq!(
                value["data"][index]["embedding"],
                json!([index as f64 + 0.25, -0.5])
            );
        }
        {
            let seen = f.calls.lock().unwrap();
            assert_eq!(
                seen.len(),
                before + 1,
                "a batch must be one upstream request"
            );
            assert_eq!(seen.last().unwrap().path, "/v1/embeddings?trace=kept");
            assert_eq!(
                seen.last().unwrap().body.as_ref(),
                original
                    .replace("bonded-embedding", "upstream-embed")
                    .as_bytes()
            );
            assert!(!seen.last().unwrap().headers.contains_key("authorization"));
        }
        f.assert_idle().await;
    }
}

#[tokio::test]
async fn embeddings_options_and_extensions_remain_opaque() {
    let f = Fixture::new("embeddings").await;
    let small = r#"{ "model":"bonded-embedding", "input":["typescript","rust"], "encoding_format":"base64", "previous_response_id":17, "conversation":"not-a-routing-handle", "tools":{"extension":true}, "echo":"bonded-embedding" }"#;
    for body in [
        small.to_string(),
        small.replace("typescript", &"t".repeat(1024 * 1024)),
    ] {
        let response = f.post("/v1/embeddings", body.clone()).send().await.unwrap();
        assert_eq!(response.status(), 200);
        let output: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
        assert_eq!(output["data"].as_array().unwrap().len(), 2);
        assert_eq!(output["data"][0]["embedding"], "AACAPwAAAEA=");
        assert_eq!(
            f.calls.lock().unwrap().last().unwrap().body.as_ref(),
            body.replacen("bonded-embedding", "upstream-embed", 1)
                .as_bytes()
        );
        assert_eq!(f.app.status()["state_entries"], 0);
        f.assert_idle().await;
    }
}

#[tokio::test]
async fn embeddings_native_errors_and_invalid_json_are_preserved_or_rejected() {
    let f = Fixture::new("embeddings").await;
    let response = f
        .post(
            "/v1/embeddings",
            r#"{"model":"bonded-embedding","input":[]}"#,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(response.headers()["x-native"], "empty-batch");
    assert_eq!(
        response.text().await.unwrap(),
        "{\"error\":{\"message\":\"input cannot be empty\",\"type\":\"native_error\"}}"
    );
    let response = f
        .post(
            "/v1/embeddings",
            r#"{"model":"bonded-embedding","input":"native-rate-limit"}"#,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers()["retry-after"], "7");
    assert_eq!(
        response.text().await.unwrap(),
        "{ \"error\": {\"message\":\"native limit\"} }"
    );
    let calls = f.calls.lock().unwrap().len();
    for body in [
        "{",
        r#"{"model":"bonded-embedding","model":"another","input":"text"}"#,
        r#"{"input":"text"}"#,
    ] {
        assert_eq!(
            f.post("/v1/embeddings", body)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    assert_eq!(f.calls.lock().unwrap().len(), calls);
    let raw = f
        .post(
            "/v1/embeddings",
            r#"{"model":"upstream-embed","input":"text"}"#,
        )
        .header("x-apibond-raw", "true")
        .header("x-apibond-model", "bonded-embedding")
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status(), 400);
    f.assert_idle().await;
}
