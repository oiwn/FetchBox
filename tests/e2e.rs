//! End-to-end integration tests for FetchBox
//!
//! These tests verify the complete system flow:
//! 1. Publish DownloadTask to Iggy
//! 2. Worker consumes task from Iggy
//! 3. Worker downloads resource from HTTP server
//! 4. Worker uploads to storage
//! 5. Verify content matches original
//!
//! Prerequisites:
//! - Iggy server running on localhost:8090
//! - Run via: `just test-e2e`

use axum::{routing::get, Router};
use bytes::Bytes;
use fetchbox::messaging::iggy::{ConsumerConfig, FetchboxConsumer, FetchboxProducer, RetryPolicy};
use fetchbox::proto::{DownloadTask, HttpHeader};
use fetchbox::storage::StorageClient;
use fetchbox::streams::{JOBS_TASKS, CONSUMER_GROUP_WORKERS};
use fetchbox::worker::runner;
use iggy::client::{Client, StreamClient, TopicClient};
use iggy::clients::client::IggyClient;
use iggy::compression::compression_algorithm::CompressionAlgorithm;
use iggy::identifier::Identifier;
use iggy::utils::expiry::IggyExpiry;
use iggy::utils::topic_size::MaxTopicSize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::time::{sleep, timeout, Duration};
use uuid::Uuid;

/// Test context holding all shared resources
struct E2EContext {
    producer: Arc<FetchboxProducer>,
    consumer: FetchboxConsumer<DownloadTask>,
    storage: Arc<StorageClient>,
    mock_server_url: String,
    iggy_endpoint: String,
}

impl E2EContext {
    /// Initialize test context
    async fn setup() -> Result<Self, Box<dyn std::error::Error>> {
        let iggy_endpoint = std::env::var("IGGY_ENDPOINT")
            .unwrap_or_else(|_| "iggy://iggy:iggy@127.0.0.1:8090".to_string());

        println!("Connecting to Iggy at: {}", iggy_endpoint);

        // Connect producer
        let producer = FetchboxProducer::connect(&iggy_endpoint, RetryPolicy::default())
            .await
            .map_err(|e| format!("Failed to connect producer: {}", e))?;

        println!("Producer connected");

        // Setup streams
        setup_iggy_streams(&producer, &iggy_endpoint).await?;

        println!("Streams configured");

        // Connect consumer
        let consumer_config = ConsumerConfig {
            stream: "jobs".to_string(),
            topic: "tasks".to_string(),
            consumer_group: CONSUMER_GROUP_WORKERS.to_string(),
            batch_size: 10,
            auto_commit: true,
        };

        let consumer = FetchboxConsumer::<DownloadTask>::connect(&iggy_endpoint, consumer_config)
            .await
            .map_err(|e| format!("Failed to connect consumer: {}", e))?;

        println!("Consumer connected");

        // Create in-memory storage
        let storage = Arc::new(StorageClient::in_memory());

        // Start mock HTTP server
        let mock_server_url = start_mock_server().await?;

        println!("Mock HTTP server started at: {}", mock_server_url);

        Ok(Self {
            producer: Arc::new(producer),
            consumer,
            storage,
            mock_server_url,
            iggy_endpoint,
        })
    }

    /// Create a test download task
    fn create_task(&self, resource_id: &str, file_path: &str) -> DownloadTask {
        let job_id = format!("test-job-{}", Uuid::new_v4());
        let url = format!("{}/{}", self.mock_server_url, file_path);

        DownloadTask {
            job_id: job_id.clone(),
            job_type: "test".to_string(),
            resource_id: resource_id.to_string(),
            url,
            headers: vec![],
            proxy_hint: None,
            storage_hint: None,
            attributes: None,
            manifest_key: format!("s3://test-bucket/manifests/{}.json", job_id),
            attempt: 1,
            tenant: "test-tenant".to_string(),
            trace_id: Uuid::new_v4().to_string(),
        }
    }
}

/// Setup Iggy streams (idempotent)
async fn setup_iggy_streams(
    producer: &FetchboxProducer,
    endpoint: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Connect raw Iggy client for stream management
    let endpoint_without_protocol = endpoint.strip_prefix("iggy://").unwrap_or(endpoint);
    
    // Extract server address (remove credentials if present)
    let server_address = if let Some(at_pos) = endpoint_without_protocol.find('@') {
        &endpoint_without_protocol[at_pos + 1..]
    } else {
        endpoint_without_protocol
    }.to_string();
    
    let client = IggyClient::builder()
        .with_tcp()
        .with_server_address(server_address)
        .build()
        .map_err(|e| format!("Failed to create Iggy client: {}", e))?;

    Client::connect(&client)
        .await
        .map_err(|e| format!("Failed to connect to Iggy: {}", e))?;

    // Authenticate with default credentials
    use iggy::client::UserClient;
    UserClient::login_user(&client, "iggy", "iggy")
        .await
        .map_err(|e| format!("Authentication failed: {}", e))?;

    // Parse stream and topic from "jobs.tasks"
    let stream_name = "jobs";
    let topic_name = "tasks";

    // Create stream if it doesn't exist
    let stream_id = Identifier::from_str_value(stream_name)
        .map_err(|e| format!("Invalid stream name: {}", e))?;

    match StreamClient::get_stream(&client, &stream_id).await {
        Ok(_) => {
            println!("Stream '{}' already exists", stream_name);
        }
        Err(_) => {
            println!("Creating stream '{}'", stream_name);
            StreamClient::create_stream(&client, stream_name, Some(1))
                .await
                .map_err(|e| format!("Failed to create stream: {}", e))?;
        }
    }

    // Create topic if it doesn't exist
    let topic_id = Identifier::from_str_value(topic_name)
        .map_err(|e| format!("Invalid topic name: {}", e))?;

    match TopicClient::get_topic(&client, &stream_id, &topic_id).await {
        Ok(_) => {
            println!("Topic '{}.{}' already exists", stream_name, topic_name);
        }
        Err(_) => {
            println!("Creating topic '{}.{}'", stream_name, topic_name);
            TopicClient::create_topic(
                &client,
                &stream_id,
                topic_name,
                8,
                CompressionAlgorithm::None,
                Some(1),
                Some(1),
                IggyExpiry::ServerDefault,
                MaxTopicSize::ServerDefault,
            )
            .await
            .map_err(|e| format!("Failed to create topic: {}", e))?;
        }
    }

    // Create additional streams for status, logs, dlq (simplified for MVP)
    // For now we only need jobs.tasks to test the worker

    Client::disconnect(&client)
        .await
        .map_err(|e| format!("Failed to disconnect: {}", e))?;

    Ok(())
}

/// Start embedded mock HTTP server serving test fixtures
async fn start_mock_server() -> Result<String, Box<dyn std::error::Error>> {
    let app = Router::new()
        .route("/sample.txt", get(serve_sample_txt))
        .route("/image.bin", get(serve_image_bin))
        .route("/large.txt", get(serve_large_txt))
        .route("/health", get(|| async { "OK" }));

    // Bind to random available port
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;

    // Spawn server in background
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Wait a bit for server to start
    sleep(Duration::from_millis(100)).await;

    Ok(format!("http://{}", bound_addr))
}

/// Handler for /sample.txt
async fn serve_sample_txt() -> Bytes {
    Bytes::from_static(include_bytes!("fixtures/sample.txt"))
}

/// Handler for /image.bin
async fn serve_image_bin() -> Bytes {
    Bytes::from_static(include_bytes!("fixtures/image.bin"))
}

/// Handler for /large.txt
async fn serve_large_txt() -> Bytes {
    Bytes::from_static(include_bytes!("fixtures/large.txt"))
}

/// Test: Basic publish and consume
#[tokio::test]
async fn test_publish_and_consume() {
    let mut ctx = E2EContext::setup()
        .await
        .expect("Failed to setup test context");

    // Create and publish task
    let task = ctx.create_task("res-1", "sample.txt");
    let job_id = task.job_id.clone();

    println!("Publishing task for job: {}", job_id);
    ctx.producer
        .publish_download_task(&task)
        .await
        .expect("Failed to publish task");

    println!("Task published, waiting for consumption...");

    // Poll for messages with timeout
    let result = timeout(Duration::from_secs(10), async {
        loop {
            match ctx.consumer.poll().await {
                Ok(messages) => {
                    if !messages.is_empty() {
                        println!("Consumed {} message(s)", messages.len());
                        return messages;
                    }
                }
                Err(e) => {
                    eprintln!("Poll error: {}", e);
                }
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    let messages = result.expect("Timeout waiting for messages");
    assert!(!messages.is_empty(), "Should receive at least one message");

    let received_task = &messages[0].payload;
    assert_eq!(received_task.job_id, job_id);
    assert_eq!(received_task.resource_id, "res-1");

    println!("✓ Test passed: publish and consume");
}

/// Test: End-to-end download workflow
#[tokio::test]
async fn test_e2e_download_workflow() {
    let mut ctx = E2EContext::setup()
        .await
        .expect("Failed to setup test context");

    // Create and publish task
    let task = ctx.create_task("res-download-1", "sample.txt");
    let job_id = task.job_id.clone();
    let resource_id = task.resource_id.clone();

    println!("Publishing task for job: {}", job_id);
    ctx.producer
        .publish_download_task(&task)
        .await
        .expect("Failed to publish task");

    // Poll and process task
    println!("Polling for task...");
    let result = timeout(Duration::from_secs(10), async {
        loop {
            match ctx.consumer.poll().await {
                Ok(messages) => {
                    if !messages.is_empty() {
                        return messages;
                    }
                }
                Err(e) => {
                    eprintln!("Poll error: {}", e);
                }
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    let messages = result.expect("Timeout waiting for messages");
    let consumed_task = messages[0].payload.clone();

    println!("Task consumed, processing download...");

    // Process task using worker runner
    runner::process_task(
        consumed_task,
        ctx.storage.clone(),
        ctx.producer.clone(),
        None, // No proxy
    )
    .await
    .expect("Failed to process task");

    println!("Task processed, verifying storage...");

    // Verify file in storage
    let expected_key = format!("resources/test/{}/{}", job_id, resource_id);
    let stored_data = ctx
        .storage
        .download(&expected_key)
        .await
        .expect("Failed to download from storage");

    // Compare with original fixture
    let original_data = include_bytes!("fixtures/sample.txt");
    assert_eq!(
        stored_data.as_slice(),
        original_data,
        "Stored data should match original"
    );

    println!("✓ Test passed: end-to-end download workflow");
}

/// Test: Download binary file
#[tokio::test]
async fn test_download_binary_file() {
    let mut ctx = E2EContext::setup()
        .await
        .expect("Failed to setup test context");

    let task = ctx.create_task("binary-res-1", "image.bin");
    let job_id = task.job_id.clone();
    let resource_id = task.resource_id.clone();

    // Publish and consume
    ctx.producer
        .publish_download_task(&task)
        .await
        .expect("Failed to publish");

    let result = timeout(Duration::from_secs(10), async {
        loop {
            match ctx.consumer.poll().await {
                Ok(messages) if !messages.is_empty() => return messages,
                _ => sleep(Duration::from_millis(100)).await,
            }
        }
    })
    .await
    .expect("Timeout");

    // Process
    runner::process_task(result[0].payload.clone(), ctx.storage.clone(), ctx.producer.clone(), None)
        .await
        .expect("Failed to process");

    // Verify binary data
    let expected_key = format!("resources/test/{}/{}", job_id, resource_id);
    let stored_data = ctx.storage.download(&expected_key).await.expect("Not found");

    let original_data = include_bytes!("fixtures/image.bin");
    assert_eq!(stored_data.as_slice(), original_data);

    println!("✓ Test passed: binary file download");
}

/// Test: Multiple resources in parallel
#[tokio::test]
async fn test_multiple_resources() {
    let mut ctx = E2EContext::setup()
        .await
        .expect("Failed to setup test context");

    // Create 3 tasks
    let tasks = vec![
        ctx.create_task("multi-1", "sample.txt"),
        ctx.create_task("multi-2", "image.bin"),
        ctx.create_task("multi-3", "large.txt"),
    ];

    // Publish all tasks
    for task in &tasks {
        ctx.producer
            .publish_download_task(task)
            .await
            .expect("Failed to publish");
    }

    println!("Published {} tasks", tasks.len());

    // Consume and process all
    let mut processed = 0;
    let result = timeout(Duration::from_secs(15), async {
        while processed < tasks.len() {
            match ctx.consumer.poll().await {
                Ok(messages) => {
                    for msg in messages {
                        println!("Processing task: {}", msg.payload.resource_id);
                        runner::process_task(
                            msg.payload,
                            ctx.storage.clone(),
                            ctx.producer.clone(),
                            None,
                        )
                        .await
                        .expect("Failed to process");
                        processed += 1;
                    }
                }
                Err(e) => eprintln!("Poll error: {}", e),
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    assert!(result.is_ok(), "Should process all tasks within timeout");

    println!("✓ Test passed: multiple resources");
}

/// Test: Custom HTTP headers
#[tokio::test]
async fn test_custom_headers() {
    let mut ctx = E2EContext::setup()
        .await
        .expect("Failed to setup test context");

    let mut task = ctx.create_task("headers-res-1", "sample.txt");
    task.headers = vec![
        HttpHeader {
            name: "X-Custom-Header".to_string(),
            value: "test-value".to_string(),
        },
        HttpHeader {
            name: "User-Agent".to_string(),
            value: "FetchBoxTest/1.0".to_string(),
        },
    ];

    // Publish and consume
    ctx.producer
        .publish_download_task(&task)
        .await
        .expect("Failed to publish");

    let result = timeout(Duration::from_secs(10), async {
        loop {
            match ctx.consumer.poll().await {
                Ok(messages) if !messages.is_empty() => return messages,
                _ => sleep(Duration::from_millis(100)).await,
            }
        }
    })
    .await
    .expect("Timeout");

    // Process
    runner::process_task(result[0].payload.clone(), ctx.storage.clone(), ctx.producer.clone(), None)
        .await
        .expect("Failed to process");

    // Just verify it completed (mock server doesn't validate headers)
    let expected_key = format!("resources/test/{}/{}", task.job_id, task.resource_id);
    ctx.storage
        .download(&expected_key)
        .await
        .expect("Should be in storage");

    println!("✓ Test passed: custom headers");
}
