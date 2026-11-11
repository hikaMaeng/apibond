use apibond::{
    App,
    config::{Config, InboundAuth},
};
use clap::Parser;
use std::{io::Write, path::PathBuf, time::Duration};

use axum::serve::ListenerExt;
#[derive(Parser)]
#[command(
    version,
    about = "Session-affine native OpenAI / Anthropic / Responses API bonding"
)]
struct Args {
    #[arg(short, long, default_value = "apibond.json")]
    config: PathBuf,
    #[arg(
        long,
        help = "Validate configuration and required environment variables, then exit"
    )]
    check: bool,
    #[arg(
        long,
        help = "Create an example config (never overwrites an existing file)"
    )]
    init: bool,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("apibond: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.init {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args.config)?;
        file.write_all(include_str!("../apibond.example.json").as_bytes())?;
        println!("Created {}", args.config.display());
        return Ok(());
    }
    let config = Config::load(&args.config).map_err(std::io::Error::other)?;
    if args.check {
        println!(
            "Configuration valid: {} models, {} backends",
            config.models.len(),
            config.backends.len()
        );
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.runtime.threads())
        .event_interval(8)
        .global_queue_interval(8)
        .enable_all()
        .build()?
        .block_on(serve(config))
}
async fn serve(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "apibond=info".into()),
        )
        .init();
    if matches!(config.inbound_auth, InboundAuth::None) {
        tracing::warn!("inbound authentication is explicitly disabled");
    }
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let shutdown_secs = config.transport.shutdown_timeout_secs;
    let app = App::new(config).map_err(std::io::Error::other)?;
    let maintenance = App::spawn_maintenance(&app);
    tracing::info!(address=%listener.local_addr()?,"APIBond listening");
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let listener = listener.tap_io(|socket| {
        if let Err(error) = socket.set_nodelay(true) {
            tracing::warn!(%error,"cannot enable TCP_NODELAY");
        }
    });
    let serve = axum::serve(listener, apibond::router(app)).with_graceful_shutdown(async {
        let _ = shutdown_rx.await;
    });
    let mut server = Box::pin(std::future::IntoFuture::into_future(serve));
    tokio::select! {
        result=&mut server=>result?,
        _=shutdown_signal()=>{
            tracing::info!("draining requests");let _=shutdown_tx.send(());
            if tokio::time::timeout(Duration::from_secs(shutdown_secs),&mut server).await.is_err() {tracing::warn!("shutdown drain deadline elapsed");}
        }
    }
    maintenance.abort();
    Ok(())
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {_=tokio::signal::ctrl_c()=>(),_=term.recv()=>()};
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
