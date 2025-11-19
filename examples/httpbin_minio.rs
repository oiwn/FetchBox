//! Run this example with `cargo run --example httpbin_minio`.
//! It boots the FetchBox API + workers locally, targets a local MinIO
//! instance, and prints out the S3 URIs for both the manifest and the
//! downloaded resources.

use std::env;
use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Datelike, Utc};
use fetchbox::api::models::{JobAcceptedResponse, JobSnapshot, JobStatus};
use fetchbox::api::run_with_config_until;
use fetchbox::config::Config;
use fetchbox::ledger::LedgerStorage;
use fetchbox::queue::{DlqStorage, TasksStorage};
use reqwest::Client;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::time::{Instant, sleep};
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

const MANIFEST_JSON: &str = include_str!("manifests/httpbin.json");
const CONFIG_TEMPLATE: &str = include_str!("../config/httpbin.minio.toml");
const TENANT: &str = "httpbin-minio";
const STORAGE_FOLDER: &str = "httpbin_minio";
const TIMEOUT_RESOURCE_NAME: &str = "timeout.json";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

    println!(
        "
FetchBox httpbin + MinIO walkthrough

Prereqs:
  1. Start MinIO locally:   `minio server tmp/data/`
  2. Configure the client:  `mc alias set local http://127.0.0.1:9000 minioadmin minioadmin`
  3. Create the bucket:     `mc mb local/fetchbox-httpbin || true`
  4. Export credentials:    `export S3_ACCESS_KEY=minioadmin; export S3_SECRET_KEY=minioadmin`
Then run: `cargo run --example httpbin_minio`
"
    );

    ensure_s3_env()?;

    let mut manifest: Value = serde_json::from_str(MANIFEST_JSON)?;
    inject_timeout_resource(&mut manifest)?;
    let storage_prefix = rewrite_manifest_storage(&mut manifest)?;
    let runtime = ExampleRuntime::bootstrap()?;
    let config = runtime.load_config()?;
    let bucket = config.storage.bucket.clone();

    println!("Artifacts will be written under s3://{bucket}/{storage_prefix}/");

    let address = reserve_loopback()?;
    let base_url = format!("http://{}", address);

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server_addr = address;
    let server_task = tokio::spawn({
        let config = config.clone();
        async move {
            if let Err(err) =
                run_with_config_until(server_addr, config, async move {
                    let _ = shutdown_rx.await;
                })
                .await
            {
                eprintln!("FetchBox server exited with error: {err}");
            }
        }
    });

    let client = Client::builder().build()?;

    wait_for_health(&client, &base_url).await?;
    println!(
        "Submitting manifest with {} resources...",
        manifest["resources"]
            .as_array()
            .map(|arr| arr.len())
            .unwrap_or(0)
    );
    let job_id = submit_manifest(&client, &base_url, &manifest).await?;
    println!("Job {job_id} accepted. Polling for completion...\n");

    let snapshot = wait_for_job(&client, &base_url, &job_id).await?;
    println!(
        "Job finished with status {:?}:\n{}",
        snapshot.status,
        serde_json::to_string_pretty(&snapshot)?
    );

    if snapshot.resource_failed > 0 {
        println!(
            "\nNote: {} resource(s) failed ({} is expected to time out).",
            snapshot.resource_failed, TIMEOUT_RESOURCE_NAME
        );
    }

    println!("\nMinIO artifact locations:");
    print_minio_locations(&bucket, &storage_prefix, &manifest, &snapshot);

    println!("\nPersisted Fjall state (ledger + queue):");
    dump_state(&runtime, &job_id)?;

    println!("\nReminder: inspect objects with the MinIO client:");
    print_mc_instructions(&bucket, &storage_prefix);

    println!("\nShutting everything down...");
    let _ = shutdown_tx.send(());
    if let Err(err) = server_task.await {
        eprintln!("Failed to join server task: {err}");
    }
    println!("Example complete. Temporary data directories were cleaned up.");

    Ok(())
}

fn ensure_s3_env() -> Result<(), Box<dyn std::error::Error>> {
    fn read_var(primary: &str, fallback: &str) -> Result<String, String> {
        env::var(primary)
            .or_else(|_| env::var(fallback))
            .map_err(|_| {
                format!(
                    "Missing {primary} (or {fallback}). Export MinIO credentials before running."
                )
            })
    }

    let _access = read_var("S3_ACCESS_KEY", "AWS_ACCESS_KEY_ID")?;
    let _secret = read_var("S3_SECRET_KEY", "AWS_SECRET_ACCESS_KEY")?;
    Ok(())
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::builder()
            .with_default_directive(LevelFilter::INFO.into())
            .parse_lossy(
                "lsm_tree=warn,lsm_tree::tree=warn,lsm_tree::manifest=warn",
            )
    });

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init()
        .ok();
}

async fn wait_for_health(
    client: &Client,
    base_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
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

async fn submit_manifest(
    client: &Client,
    base_url: &str,
    manifest: &Value,
) -> Result<String, Box<dyn std::error::Error>> {
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
) -> Result<JobSnapshot, Box<dyn std::error::Error>> {
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
                if attempts > 60 {
                    return Err(
                        "Job did not finish within the polling window".into()
                    );
                }
                sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

fn rewrite_manifest_storage(
    manifest: &mut Value,
) -> Result<String, Box<dyn std::error::Error>> {
    let storage = manifest
        .get_mut("storage")
        .and_then(|value| value.as_object_mut())
        .ok_or_else(|| "manifest missing storage configuration".to_string())?;

    let now = Utc::now();
    let folder = format!(
        "{:04}/{:02}/{:02}/{}",
        now.year(),
        now.month(),
        now.day(),
        STORAGE_FOLDER
    );

    storage.insert(
        "resource_key_prefix".to_string(),
        Value::String(format!("{folder}/")),
    );
    storage.insert(
        "manifest_file".to_string(),
        Value::String("manifest.json".to_string()),
    );

    Ok(folder)
}

fn inject_timeout_resource(
    manifest: &mut Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let resources = manifest
        .get_mut("resources")
        .and_then(|value| value.as_array_mut())
        .ok_or_else(|| "manifest missing resources array".to_string())?;

    let already_present = resources.iter().any(|resource| {
        resource
            .get("name")
            .and_then(|name| name.as_str())
            .map(|name| name == TIMEOUT_RESOURCE_NAME)
            .unwrap_or(false)
    });

    if !already_present {
        resources.push(json!({
            "name": TIMEOUT_RESOURCE_NAME,
            "url": "https://httpbin.org/delay/10",
            "headers": {
                "Accept": "application/json",
                "X-Demo-Scenario": "timeout"
            },
            "tags": {
                "purpose": "demo",
                "httpbin_endpoint": "/delay/10",
                "expected": "timeout"
            }
        }));
    }

    Ok(())
}

fn print_minio_locations(
    bucket: &str,
    storage_prefix: &str,
    manifest: &Value,
    snapshot: &JobSnapshot,
) {
    println!("  Manifest: {}", snapshot.manifest_key);
    println!(
        "  Shared prefix: s3://{bucket}/{storage_prefix}/ (manifest + resources)"
    );

    let resource_names = manifest
        .get("resources")
        .and_then(|value| value.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|resource| {
                    resource.get("name").and_then(|n| n.as_str())
                })
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if resource_names.is_empty() {
        println!("  No resources found in manifest");
        return;
    }

    println!("  Resources:");
    for name in &resource_names {
        println!("    - s3://{bucket}/{storage_prefix}/{name}");
    }

    if resource_names
        .iter()
        .any(|name| name == TIMEOUT_RESOURCE_NAME)
    {
        println!(
            "  Note: {TIMEOUT_RESOURCE_NAME} intentionally times out to demo failure handling."
        );
    }
}

fn dump_state(
    runtime: &ExampleRuntime,
    job_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let ledger = LedgerStorage::open(runtime.ledger_path())?;
    let jobs = ledger.list_jobs(16)?;
    println!(
        "  Ledger jobs ({} total, highlighting job {}):",
        jobs.len(),
        job_id
    );
    for job in jobs {
        println!(
            "    - {} :: {:?} ({} succeeded / {} total)",
            job.job_id, job.status, job.resource_completed, job.resource_total
        );
    }

    let tasks = TasksStorage::open(runtime.queue_path())?;
    let persisted_tasks = tasks.list(32)?;
    println!("  Queue tasks ({} entries):", persisted_tasks.len());
    for (task_id, task) in persisted_tasks {
        println!(
            "    - {} :: job={} resource={} url={}",
            task_id, task.job_id, task.resource_id, task.url
        );
    }

    let dlq = DlqStorage::open(runtime.dlq_path())?;
    let dlq_entries = dlq.list(32)?;
    if dlq_entries.is_empty() {
        println!("  DLQ entries: none\n");
    } else {
        println!("  DLQ entries ({}):", dlq_entries.len());
        for (task_id, entry) in dlq_entries {
            if let Some(task) = entry.task {
                println!(
                    "    - {} :: job={} resource={} failure={} attempts={}",
                    task_id,
                    task.job_id,
                    task.resource_id,
                    entry.failure_code,
                    entry.attempts
                );
            } else {
                println!(
                    "    - {} :: failure={} attempts={}",
                    task_id, entry.failure_code, entry.attempts
                );
            }
        }
        println!();
    }

    Ok(())
}

fn print_mc_instructions(bucket: &str, storage_prefix: &str) {
    println!("  mc ls local/{bucket}");
    println!("  mc ls local/{bucket}/{storage_prefix}/");
    println!("  mc cat local/{bucket}/{storage_prefix}/anything.json");
    println!(
        "  mc cat local/{bucket}/{storage_prefix}/{TIMEOUT_RESOURCE_NAME}  # expected failure"
    );
}

fn reserve_loopback() -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(addr)
}

struct ExampleRuntime {
    _temp_dir: TempDir,
    config_path: PathBuf,
    ledger_path: PathBuf,
    queue_path: PathBuf,
}

impl ExampleRuntime {
    fn bootstrap() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let ledger_path = temp_dir.path().join("ledger");
        let queue_path = temp_dir.path().join("queue");
        fs::create_dir_all(&ledger_path)?;
        fs::create_dir_all(&queue_path)?;

        let rendered = CONFIG_TEMPLATE
            .replace("__LEDGER_DIR__", ledger_path.to_string_lossy().as_ref())
            .replace("__QUEUE_DIR__", queue_path.to_string_lossy().as_ref());

        let config_path = temp_dir.path().join("fetchbox.httpbin.minio.toml");
        fs::write(&config_path, rendered)?;

        println!(
            "Using temp ledger at {}\nUsing temp queue at {}",
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

    fn ledger_path(&self) -> &Path {
        &self.ledger_path
    }

    fn queue_path(&self) -> &Path {
        &self.queue_path
    }

    fn dlq_path(&self) -> PathBuf {
        self.queue_path.with_file_name("dlq")
    }
}
