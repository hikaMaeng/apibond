//! Loopback fixture benchmark, with the proxy isolated in a native child process.
use axum::serve::ListenerExt;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::Request,
    response::{IntoResponse, Response},
};
use futures_util::{StreamExt, future::join_all};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Proxy(Child);
impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn upstream(request: Request) -> Response {
    let streaming = request.uri().query() == Some("stream=1");
    let slow = request.uri().query() == Some("slow=1");
    let mut input = request.into_body().into_data_stream();
    let mut count = 0usize;
    while let Some(chunk) = input.next().await {
        let _ = chunk.unwrap();
        count += 1;
        if count.is_multiple_of(4) {
            tokio::task::yield_now().await;
        }
    }
    if slow {
        let chunk = Bytes::from(vec![b'x'; 65536]);
        let output = async_stream::stream! {for _ in 0..10000 {yield Ok::<_,std::io::Error>(chunk.clone());}};
        return (
            [("content-type", "application/octet-stream")],
            Body::from_stream(output),
        )
            .into_response();
    }
    if streaming {
        let stream = async_stream::stream! {
            for _ in 0..50 {let sent=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as u64;yield Ok::<_,std::io::Error>(Bytes::from(format!("data: {{\"text\":\"fixture\",\"sent_us\":{sent}}}\n\n")));tokio::time::sleep(Duration::from_millis(2)).await;}
        };
        (
            [("content-type", "text/event-stream")],
            Body::from_stream(stream),
        )
            .into_response()
    } else {
        ([("content-type", "application/json")], "{\"ok\":true}").into_response()
    }
}
#[derive(Default)]
struct Sample {
    headers: f64,
    first: f64,
    total: f64,
    gaps: Vec<f64>,
    transit: Vec<f64>,
    producer_gaps: Vec<f64>,
    ok: bool,
}
async fn request(client: &reqwest::Client, url: &str, body: Bytes, raw: bool) -> Sample {
    let start = Instant::now();
    let mut req = client
        .post(url)
        .header("content-type", "application/json")
        .body(body);
    if raw {
        req = req
            .header("x-apibond-model", "raw")
            .header("x-apibond-raw", "true");
    }
    let Ok(response) = req.send().await else {
        return Sample::default();
    };
    let ok = response.status().is_success();
    let is_sse = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    let headers = start.elapsed().as_secs_f64() * 1000.0;
    let mut stream = response.bytes_stream();
    let mut first = None;
    let mut last = None;
    let mut gaps = Vec::new();
    let mut pending = Vec::new();
    let mut transit = Vec::new();
    let mut producer_gaps = Vec::new();
    let mut previous_sent = None;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            return Sample::default();
        };
        let now = Instant::now();
        if is_sse {
            pending.extend_from_slice(&chunk);
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line = pending.drain(..=end).collect::<Vec<_>>();
                if let Some(payload) = line.strip_prefix(b"data: ")
                    && let Ok(value) = serde_json::from_slice::<Value>(payload)
                    && let Some(sent) = value["sent_us"].as_u64()
                {
                    let received = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_micros() as u64;
                    transit.push(received.saturating_sub(sent) as f64 / 1000.0);
                    if let Some(previous) = previous_sent {
                        producer_gaps.push(sent.saturating_sub(previous) as f64 / 1000.0);
                    }
                    previous_sent = Some(sent);
                }
            }
        }
        first.get_or_insert(now);
        if let Some(previous) = last {
            gaps.push(now.duration_since(previous).as_secs_f64() * 1000.0);
        }
        last = Some(now);
    }
    Sample {
        headers,
        first: first
            .unwrap_or_else(Instant::now)
            .duration_since(start)
            .as_secs_f64()
            * 1000.0,
        total: start.elapsed().as_secs_f64() * 1000.0,
        gaps,
        transit,
        producer_gaps,
        ok,
    }
}
fn percentiles(mut numbers: Vec<f64>) -> Value {
    if numbers.is_empty() {
        return Value::Null;
    }
    numbers.sort_by(f64::total_cmp);
    let p = |q: f64| numbers[((numbers.len() - 1) as f64 * q).round() as usize];
    json!({"p50":p(0.5),"p95":p(0.95),"p99":p(0.99)})
}
fn statistics(samples: &[Sample], elapsed: f64) -> Value {
    json!({"samples":samples.len(),"errors":samples.iter().filter(|s|!s.ok).count(),"response_headers_ms":percentiles(samples.iter().filter(|s|s.ok).map(|s|s.headers).collect()),"first_byte_ms":percentiles(samples.iter().filter(|s|s.ok).map(|s|s.first).collect()),"total_ms":percentiles(samples.iter().filter(|s|s.ok).map(|s|s.total).collect()),"inter_chunk_ms":percentiles(samples.iter().flat_map(|s|s.gaps.iter().copied()).collect()),"producer_inter_chunk_ms":percentiles(samples.iter().flat_map(|s|s.producer_gaps.iter().copied()).collect()),"emission_to_receive_ms":percentiles(samples.iter().flat_map(|s|s.transit.iter().copied()).collect()),"batch_requests_per_sec":samples.iter().filter(|s|s.ok).count() as f64/elapsed})
}
fn process_memory(pid: u32) -> Value {
    #[cfg(windows)]
    {
        let script = format!(
            "$p=Get-Process -Id {pid}; [PSCustomObject]@{{working_set_bytes=$p.WorkingSet64;peak_working_set_bytes=$p.PeakWorkingSet64;cpu_seconds=$p.CPU}} | ConvertTo-Json -Compress"
        );
        if let Ok(output) = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
        {
            return serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
            let size = |key: &str| {
                status
                    .lines()
                    .find(|l| l.starts_with(key))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|n| n.parse::<u64>().ok())
                    .map(|n| n * 1024)
            };
            return json!({"working_set_bytes":size("VmRSS:"),"peak_working_set_bytes":size("VmHWM:")});
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
        {
            return json!({"observed_rss_bytes":String::from_utf8_lossy(&output.stdout).trim().parse::<u64>().ok().map(|v|v*1024)});
        }
    }
    Value::Null
}

#[tokio::main(worker_threads = 8)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--mock-upstream") {
        let address = std::env::args()
            .nth(2)
            .ok_or("missing mock listen address")?;
        let listener = tokio::net::TcpListener::bind(address).await?;
        axum::serve(
            listener.tap_io(|s| {
                let _ = s.set_nodelay(true);
            }),
            Router::new().fallback(upstream),
        )
        .await?;
        return Ok(());
    }
    let stress_only = std::env::args().any(|a| a == "--stress-only");
    let binary = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(if cfg!(windows) {
                "target/release/apibond.exe"
            } else {
                "target/release/apibond"
            })
        });
    let reserved = std::net::TcpListener::bind("127.0.0.1:0")?;
    let upstream_address = reserved.local_addr()?;
    drop(reserved);
    let upstream_url = format!("http://{upstream_address}");
    let mock = Proxy(
        Command::new(std::env::current_exe()?)
            .args(["--mock-upstream", &upstream_address.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let reservation = std::net::TcpListener::bind("127.0.0.1:0")?;
    let listen = reservation.local_addr()?;
    drop(reservation);
    let mut cfg: Value = serde_json::from_str(include_str!("../apibond.example.json"))?;
    cfg["listen"] = json!(listen.to_string());
    cfg["backends"].as_object_mut().unwrap().remove("server-b");
    cfg["backends"]["server-a"]["base_url"] = json!(format!("{upstream_url}/v1"));
    cfg["backends"]["server-a"]["capacity"] = json!(256);
    cfg["limits"]["buffered_json_budget_bytes"] = json!(3u64 * 1024 * 1024 * 1024);
    cfg["runtime"]["worker_threads"] = json!(if stress_only { 2 } else { 8 });
    cfg["models"] = json!({"match":{"backends":[{"backend":"server-a","model":"match"}]},"rewrite":{"backends":[{"backend":"server-a","model":"actual"}]},"raw":{"allow_raw":true,"backends":[{"backend":"server-a","model":"actual"}]}});
    std::fs::create_dir_all("work")?;
    std::fs::write("work/bench-config.json", serde_json::to_vec(&cfg)?)?;
    let proxy = Proxy(
        Command::new(&binary)
            .args(["--config", "work/bench-config.json"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let pid = proxy.0.id();
    let base = format!("http://{listen}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(256)
        .build()?;
    for _ in 0..200 {
        if client
            .get(format!("{upstream_url}/healthz"))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    for _ in 0..200 {
        if client.get(format!("{base}/healthz")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let mut results = Vec::new();
    for size in if stress_only {
        vec![]
    } else {
        vec![1024, 1024 * 1024, 16 * 1024 * 1024]
    } {
        for mode in ["match", "rewrite", "raw"] {
            let model = if mode == "raw" { "actual" } else { mode };
            let body = Bytes::from(format!(
                "{{\"model\":\"{model}\",\"data\":\"{}\"}}",
                "A".repeat(size)
            ));
            for concurrency in [1, 64] {
                let direct = format!("{upstream_url}/v1/chat/completions");
                let bonded = format!("{base}/v1/chat/completions");
                for _ in 0..3 {
                    join_all(
                        (0..concurrency).map(|_| request(&client, &direct, body.clone(), false)),
                    )
                    .await;
                    join_all(
                        (0..concurrency)
                            .map(|_| request(&client, &bonded, body.clone(), mode == "raw")),
                    )
                    .await;
                }
                let count = if size >= 16 * 1024 * 1024 { 64 } else { 256 };
                let mut d = Vec::new();
                let mut p = Vec::new();
                let mut dt = 0.0;
                let mut pt = 0.0;
                for batch in 0..(count / concurrency) {
                    for proxy_first in [batch % 2 == 0, batch % 2 != 0] {
                        let (url, raw) = if proxy_first {
                            (&bonded, mode == "raw")
                        } else {
                            (&direct, false)
                        };
                        let start = Instant::now();
                        let samples = join_all(
                            (0..concurrency).map(|_| request(&client, url, body.clone(), raw)),
                        )
                        .await;
                        let elapsed = start.elapsed().as_secs_f64();
                        if proxy_first {
                            p.extend(samples);
                            pt += elapsed;
                        } else {
                            d.extend(samples);
                            dt += elapsed;
                        }
                    }
                }
                let row = json!({"kind":"json","payload_data_bytes":size,"mode":mode,"concurrency":concurrency,"direct":statistics(&d,dt),"proxy":statistics(&p,pt),"proxy_process":process_memory(pid)});
                println!("{}", serde_json::to_string(&row)?);
                results.push(row);
            }
        }
    }
    let body = Bytes::from_static(br#"{"model":"match","stream":true}"#);
    let direct = format!("{upstream_url}/v1/chat/completions?stream=1");
    let bonded = format!("{base}/v1/chat/completions?stream=1");
    for concurrency in if stress_only { vec![] } else { vec![1, 64] } {
        let mut d = Vec::new();
        let mut p = Vec::new();
        let mut dt = 0.0;
        let mut pt = 0.0;
        for _ in 0..if concurrency == 1 { 10 } else { 1 } {
            let t = Instant::now();
            d.extend(
                join_all((0..concurrency).map(|_| request(&client, &direct, body.clone(), false)))
                    .await,
            );
            dt += t.elapsed().as_secs_f64();
            let t = Instant::now();
            p.extend(
                join_all((0..concurrency).map(|_| request(&client, &bonded, body.clone(), false)))
                    .await,
            );
            pt += t.elapsed().as_secs_f64();
        }
        let row = json!({"kind":"sse","concurrency":concurrency,"producer_chunks":50,"producer_delay_ms":2,"direct":statistics(&d,dt),"proxy":statistics(&p,pt),"proxy_process":process_memory(pid)});
        println!("{row}");
        results.push(row);
    }
    let big = Bytes::from(format!(
        "{{\"model\":\"rewrite\",\"messages\":[{{\"role\":\"user\",\"content\":[{{\"type\":\"image\",\"source\":{{\"type\":\"base64\",\"data\":\"{}\"}}}}]}}]}}",
        "A".repeat(60 * 1024 * 1024)
    ));
    let upload_url = format!("{base}/v1/messages");
    let start = Instant::now();
    let (sse, direct_sse, uploads) = tokio::join!(
        request(&client, &bonded, body.clone(), false),
        request(&client, &direct, body.clone(), false),
        join_all((0..4).map(|_| request(&client, &upload_url, big.clone(), false)))
    );
    let elapsed = start.elapsed().as_secs_f64();
    let row = json!({"kind":"mixed_multimodal_sse","native_proxy_workers":cfg["runtime"]["worker_threads"],"parallel_images":4,"image_base64_bytes":60*1024*1024,"sse":statistics(&[sse],elapsed),"direct_sse":statistics(&[direct_sse],elapsed),"uploads":statistics(&uploads,elapsed),"proxy_process":process_memory(pid)});
    println!("{row}");
    results.push(row);
    let slow_url = format!("{base}/v1/chat/completions?slow=1");
    let before = process_memory(pid);
    let held = join_all((0..8).map(|_| {
        client
            .post(&slow_url)
            .header("content-type", "application/json")
            .body(body.clone())
            .send()
    }))
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let at_one = process_memory(pid);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let at_three = process_memory(pid);
    let status = client
        .get(format!("{base}/status"))
        .send()
        .await?
        .bytes()
        .await?;
    let status: Value = serde_json::from_slice(&status)?;
    drop(held);
    let mut after = Value::Null;
    for _ in 0..200 {
        let status = client
            .get(format!("{base}/status"))
            .send()
            .await?
            .bytes()
            .await?;
        after = serde_json::from_slice(&status)?;
        if after["backends"][0]["active"] == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let row = json!({"kind":"slow_client_cancel","clients":8,"producer_bytes_per_client":65536u64*10000,"before":before,"at_one_second":at_one,"at_three_seconds":at_three,"active_while_held":status["backends"][0]["active"],"active_after_cancel":after["backends"][0]["active"]});
    println!("{row}");
    results.push(row);
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(std::fs::read(&binary)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let rustc = Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let native_version = Command::new(&binary)
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    std::fs::write(
        if stress_only {
            "work/benchmark-stress.json"
        } else {
            "work/benchmark.json"
        },
        serde_json::to_vec_pretty(
            &json!({"fixture":true,"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,"processor_identifier":std::env::var("PROCESSOR_IDENTIFIER").ok(),"host_logical_cores":std::thread::available_parallelism().map(|n|n.get()).ok(),"rustc":rustc,"native_version":native_version,"binary_sha256":digest,"proxy_config":cfg,"harness_worker_threads":8,"upstream_pid":mock.0.id(),"proxy_pid":pid,"results":results}),
        )?,
    )?;
    Ok(())
}
