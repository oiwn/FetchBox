//! Local RPS/latency probe with no external dependencies.
//! - Spins up an Axum blob fixture (serves deterministic blobs).
//! - Runs FetchBox with in-memory storage + temp Fjall/queue paths.
//! - Pushes manifests to `/jobs` and reports throughput + error counts.
//!
//! Run:
//! `cargo run --example bench_local`
//! Quiet logs:
//! `cargo run --example bench_local -- --quiet`
//! Tunables via env:
//!   BENCH_JOBS (default 20)
//!   BENCH_TASKS_PER_JOB (default 50)
//!   BENCH_BLOB_BYTES (default 1_000_000)
//!   BENCH_SUBMIT_CONCURRENCY (default 4)
//!   BENCH_WORKERS (default 8)
//!   BENCH_RATE_LIMIT (default 32)  // per worker, req/sec

use axum::{
    Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    routing::get,
};
use bytes::Bytes;
use clap::Parser;
use fetchbox::api::models::{JobAcceptedResponse, JobSnapshot, JobStatus};
use fetchbox::api::run_with_config_until;
use fetchbox::config::Config;
use reqwest::Client;
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{Mutex, Semaphore, oneshot};
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep};
use tracing_subscriber::{EnvFilter, filter::LevelFilter};
use uuid::Uuid;

const TENANT: &str = "bench-local";
type AnyError = Box<dyn std::error::Error + Send + Sync>;

// Bench config template: placeholders are filled with runtime values.
const CONFIG_TEMPLATE: &str = r#"
[server]
bind_addr = "__BIND_ADDR__"
fjall_path = "__LEDGER_DIR__"

[queue]
path = "__QUEUE_DIR__"
workers = __WORKERS__
channel_size = 256

[queue.worker]
rate_limit_per_worker = __RATE_LIMIT__
max_retries = 1
retry_backoff_ms = 200
task_timeout_ms = 10000

[storage]
provider = "local"
bucket = "bench-local"

[handlers.default]
handler = "fetchbox::handlers::DefaultHandler"
"#;

#[derive(Debug, Clone)]
struct BenchScenario {
    jobs: usize,
    tasks_per_job: usize,
    blob_bytes: usize,
    submit_concurrency: usize,
    workers: usize,
    rate_limit_per_worker: u64,
}

impl BenchScenario {
    fn from_env() -> Self {
        Self {
            jobs: read_env_usize("BENCH_JOBS", 20),
            tasks_per_job: read_env_usize("BENCH_TASKS_PER_JOB", 50),
            blob_bytes: read_env_usize("BENCH_BLOB_BYTES", 1_000_000),
            submit_concurrency: read_env_usize("BENCH_SUBMIT_CONCURRENCY", 4),
            workers: read_env_usize("BENCH_WORKERS", 8),
            rate_limit_per_worker: read_env_u64("BENCH_RATE_LIMIT", 32),
        }
    }

    fn total_tasks(&self) -> usize {
        self.jobs * self.tasks_per_job
    }

    fn total_bytes(&self) -> u128 {
        (self.total_tasks() as u128) * (self.blob_bytes as u128)
    }
}

#[derive(Serialize)]
struct ManifestPayload {
    manifest_version: &'static str,
    storage: StorageSection,
    metadata: serde_json::Value,
    resources: Vec<ResourcePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attributes: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct StorageSection {
    manifest_file: String,
    resource_key_prefix: String,
}

#[derive(Serialize)]
struct ResourcePayload {
    name: String,
    url: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    tags: BTreeMap<String, String>,
}

#[derive(Debug)]
struct BenchResult {
    elapsed: Duration,
    jobs: usize,
    tasks: usize,
    completed: usize,
    failed: usize,
    bytes: u128,
}

#[derive(Clone)]
struct BlobState {
    blobs: Arc<Mutex<HashMap<usize, Bytes>>>,
}

#[derive(Parser, Debug)]
#[command(name = "bench_local")]
struct BenchArgs {
    /// Suppress bench chatter; keep only warnings/errors.
    #[arg(short, long)]
    quiet: bool,
}

#[tokio::main]
async fn main() -> Result<(), AnyError> {
    let args = BenchArgs::parse();
    init_tracing(args.quiet);

    let scenario = BenchScenario::from_env();
    println!(
        "Starting bench-local example with {} jobs x {} tasks ({} bytes) | workers={} rate_limit={} submit_concurrency={}",
        scenario.jobs,
        scenario.tasks_per_job,
        scenario.blob_bytes,
        scenario.workers,
        scenario.rate_limit_per_worker,
        scenario.submit_concurrency,
    );

    let api_addr = reserve_loopback()?;
    let fixture_addr = reserve_loopback()?;

    let runtime = BenchRuntime::bootstrap(
        api_addr,
        scenario.workers,
        scenario.rate_limit_per_worker,
    )?;
    let config = runtime.load_config()?;

    let (fixture_shutdown_tx, fixture_shutdown_rx) = oneshot::channel();
    let fixture_task = tokio::spawn(run_blob_fixture(
        fixture_addr,
        scenario.blob_bytes,
        fixture_shutdown_rx,
    ));

    let (api_shutdown_tx, api_shutdown_rx) = oneshot::channel();
    let api_task = tokio::spawn({
        let config = config.clone();
        async move {
            if let Err(err) = run_with_config_until(api_addr, config, async move {
                let _ = api_shutdown_rx.await;
            })
            .await
            {
                eprintln!("FetchBox server exited with error: {err}");
            }
        }
    });

    let client = Client::builder().build()?;
    let api_base = format!("http://{}", api_addr);
    let fixture_base = format!("http://{}", fixture_addr);

    wait_for_fixture(&client, &fixture_base).await?;
    wait_for_health(&client, &api_base).await?;
    println!(
        "Fixture running at {fixture_base}, FetchBox API at {api_base}. Submitting workloads..."
    );

    let result = run_bench(&client, &api_base, &fixture_base, &scenario).await?;
    let throughput =
        result.completed as f64 / result.elapsed.as_secs_f64().max(0.001);
    let bytes_mb = result.bytes as f64 / 1_000_000.0;
    let mbps = bytes_mb / result.elapsed.as_secs_f64().max(0.001);

    println!(
        "\nBench summary:\n  Jobs: {}\n  Tasks: {}\n  Completed: {}\n  Failed: {}\n  Elapsed: {:.2?}\n  Throughput: {:.2} tasks/sec\n  Data: {:.2} MB total | {:.2} MB/sec",
        result.jobs,
        result.tasks,
        result.completed,
        result.failed,
        result.elapsed,
        throughput,
        bytes_mb,
        mbps,
    );

    println!("Shutting down servers...");
    let _ = fixture_shutdown_tx.send(());
    let _ = api_shutdown_tx.send(());
    let _ = fixture_task.await;
    let _ = api_task.await;

    Ok(())
}

async fn run_bench(
    client: &Client,
    api_base: &str,
    fixture_base: &str,
    scenario: &BenchScenario,
) -> Result<BenchResult, AnyError> {
    let start = Instant::now();
    let mut job_ids = Vec::with_capacity(scenario.jobs);

    let submit_semaphore = Arc::new(Semaphore::new(scenario.submit_concurrency));
    let mut submit_set = JoinSet::new();

    for job_idx in 0..scenario.jobs {
        let client = client.clone();
        let api_base = api_base.to_string();
        let manifest = build_manifest(job_idx, scenario, fixture_base);
        let sem = submit_semaphore.clone();
        submit_set.spawn(async move {
            let _permit =
                sem.acquire_owned().await.expect("submit semaphore closed");
            submit_manifest(&client, &api_base, &manifest).await
        });
    }

    while let Some(res) = submit_set.join_next().await {
        job_ids.push(res??);
    }

    let mut completed = 0usize;
    let mut failed = 0usize;

    let mut waits = JoinSet::new();
    for job_id in job_ids {
        let client = client.clone();
        let api_base = api_base.to_string();
        waits.spawn(async move { wait_for_job(&client, &api_base, &job_id).await });
    }

    while let Some(res) = waits.join_next().await {
        let snapshot = res??;
        completed += snapshot.resource_completed;
        failed += snapshot.resource_failed;
    }

    let elapsed = start.elapsed();
    Ok(BenchResult {
        elapsed,
        jobs: scenario.jobs,
        tasks: scenario.total_tasks(),
        completed,
        failed,
        bytes: scenario.total_bytes(),
    })
}

fn build_manifest(
    job_idx: usize,
    scenario: &BenchScenario,
    fixture_base: &str,
) -> ManifestPayload {
    let mut resources = Vec::with_capacity(scenario.tasks_per_job);
    for task_idx in 0..scenario.tasks_per_job {
        let name = format!("blob-job{job_idx}-task{task_idx}.bin");
        let url = format!("{fixture_base}/blob/{}/{task_idx}", scenario.blob_bytes);
        resources.push(ResourcePayload {
            name,
            url,
            headers: BTreeMap::new(),
            tags: BTreeMap::new(),
        });
    }

    let prefix = format!("bench/{}/job-{job_idx}/", Uuid::new_v4());
    ManifestPayload {
        manifest_version: "v1",
        storage: StorageSection {
            manifest_file: "manifest.json".to_string(),
            resource_key_prefix: prefix,
        },
        metadata: json!({
            "scenario": "bench_local",
            "blob_bytes": scenario.blob_bytes,
            "tasks_per_job": scenario.tasks_per_job,
        }),
        resources,
        attributes: None,
    }
}

async fn wait_for_health(client: &Client, base_url: &str) -> Result<(), AnyError> {
    let url = format!("{base_url}/operators/health");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match client.get(&url).send().await {
            Ok(response) if response.status().is_success() => return Ok(()),
            _ => {
                if Instant::now() >= deadline {
                    return Err("Timed out waiting for FetchBox API to boot".into());
                }
                sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn wait_for_fixture(client: &Client, base_url: &str) -> Result<(), AnyError> {
    let url = format!("{base_url}/health");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match client.get(&url).send().await {
            Ok(response) if response.status().is_success() => return Ok(()),
            _ => {
                if Instant::now() >= deadline {
                    return Err("Timed out waiting for blob fixture to boot".into());
                }
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn submit_manifest(
    client: &Client,
    base_url: &str,
    manifest: &ManifestPayload,
) -> Result<String, AnyError> {
    let url = format!("{base_url}/jobs");
    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("X-Fetchbox-Tenant", TENANT)
        .json(manifest)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Job submission failed: {} - {}", status, body).into());
    }

    let accepted: JobAcceptedResponse = response.json().await?;
    Ok(accepted.job_id)
}

async fn wait_for_job(
    client: &Client,
    base_url: &str,
    job_id: &str,
) -> Result<JobSnapshot, AnyError> {
    let url = format!("{base_url}/operators/jobs/{job_id}");
    let mut attempts = 0u32;

    loop {
        attempts += 1;
        let response = client.get(&url).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(
                format!("Status check failed: {} - {}", status, body).into()
            );
        }

        let snapshot: JobSnapshot = response.json().await?;
        match snapshot.status {
            JobStatus::Completed | JobStatus::Failed => return Ok(snapshot),
            JobStatus::Queued | JobStatus::InProgress => {
                if attempts > 600 {
                    return Err(
                        "Job did not finish within the polling window".into()
                    );
                }
                sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn run_blob_fixture(
    addr: SocketAddr,
    default_blob_bytes: usize,
    shutdown: oneshot::Receiver<()>,
) -> Result<(), AnyError> {
    let state = BlobState {
        blobs: Arc::new(Mutex::new(HashMap::new())),
    };
    prime_blob(&state, default_blob_bytes).await;

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/blob/{size}/{task}", get(blob_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!(
        "Blob fixture listening on http://{}",
        listener.local_addr()?
    );

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(async move {
            let _ = shutdown.await;
        })
        .await?;

    Ok(())
}

async fn blob_handler(
    AxumPath((size, _task)): AxumPath<(usize, usize)>,
    State(state): State<BlobState>,
) -> Result<Bytes, (StatusCode, &'static str)> {
    if size == 0 {
        return Err((StatusCode::BAD_REQUEST, "size must be > 0"));
    }

    if let Some(bytes) = maybe_get_blob(&state, size).await {
        return Ok(bytes);
    }

    prime_blob(&state, size).await;
    maybe_get_blob(&state, size)
        .await
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "failed to prime blob"))
}

async fn maybe_get_blob(state: &BlobState, size: usize) -> Option<Bytes> {
    let blobs = state.blobs.lock().await;
    blobs.get(&size).cloned()
}

async fn prime_blob(state: &BlobState, size: usize) {
    let mut blobs = state.blobs.lock().await;
    blobs.entry(size).or_insert_with(|| {
        let payload = vec![b'Z'; size];
        Bytes::from(payload)
    });
}

fn init_tracing(quiet: bool) {
    let env_filter = if quiet {
        EnvFilter::builder()
            .with_default_directive(LevelFilter::WARN.into())
            .parse_lossy(
                "lsm_tree=warn,lsm_tree::tree=warn,lsm_tree::manifest=warn",
            )
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .parse_lossy(
                    "lsm_tree=warn,lsm_tree::tree=warn,lsm_tree::manifest=warn",
                )
        })
    };

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init()
        .ok();
}

fn reserve_loopback() -> Result<SocketAddr, AnyError> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(addr)
}

fn read_env_usize(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

fn read_env_u64(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

struct BenchRuntime {
    _temp_dir: TempDir,
    config_path: PathBuf,
    ledger_path: PathBuf,
    queue_path: PathBuf,
}

impl BenchRuntime {
    fn bootstrap(
        api_addr: SocketAddr,
        workers: usize,
        rate_limit_per_worker: u64,
    ) -> Result<Self, AnyError> {
        let temp_dir = TempDir::new()?;
        let ledger_path = temp_dir.path().join("ledger");
        let queue_path = temp_dir.path().join("queue");
        fs::create_dir_all(&ledger_path)?;
        fs::create_dir_all(&queue_path)?;

        let rendered = CONFIG_TEMPLATE
            .replace("__LEDGER_DIR__", ledger_path.to_string_lossy().as_ref())
            .replace("__QUEUE_DIR__", queue_path.to_string_lossy().as_ref())
            .replace("__BIND_ADDR__", api_addr.to_string().as_str())
            .replace("__WORKERS__", &workers.to_string())
            .replace("__RATE_LIMIT__", &rate_limit_per_worker.to_string());

        let config_path = temp_dir.path().join("fetchbox.bench.local.toml");
        fs::write(&config_path, rendered)?;

        println!(
            "Bench config written to {} (temp)\nLedger at {}\nQueue at {}",
            config_path.display(),
            ledger_path.display(),
            queue_path.display()
        );

        Ok(Self {
            _temp_dir: temp_dir,
            config_path,
            ledger_path,
            queue_path,
        })
    }

    fn load_config(&self) -> Result<Config, fetchbox::config::ConfigError> {
        Config::load_from_path(self.config_path.clone())
    }

    #[allow(dead_code)]
    fn ledger_path(&self) -> &Path {
        &self.ledger_path
    }

    #[allow(dead_code)]
    fn queue_path(&self) -> &Path {
        &self.queue_path
    }
}
