# FetchBox Integration Guide

This guide explains how to integrate FetchBox into third-party services for distributed download orchestration.

## Overview

FetchBox is a self-contained download orchestration service that accepts job manifests via HTTP API, distributes download tasks to an embedded worker pool, and stores results in S3-compatible storage while tracking state in Fjall databases.

### Key Integration Benefits

- **Fire-and-forget processing** - Submit jobs and track via `job_id`
- **Horizontal scalability** - Multiple instances behind load balancer
- **Persistent state** - Jobs survive restarts via embedded Fjall databases
- **Customizable processing** - Extensible handler system for business logic
- **Observability** - Built-in metrics, logs, and failure tracking

## Integration Architecture

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│  Your Service   │────│    FetchBox     │────│   S3 Storage    │
│                 │    │                 │    │                 │
│ • Submit jobs   │    │ • Process jobs  │    │ • Store results │
│ • Track status  │    │ • Download      │    │ • Artifacts     │
│ • Retrieve      │    │ • Retry logic   │    │ • Metadata      │
│   artifacts     │    │ • DLQ handling  │    │                 │
└─────────────────┘    └─────────────────┘    └─────────────────┘
```

## Quick Start Integration

### 1. Configure FetchBox

Create a configuration file (`fetchbox.toml`):

```toml
[server]
bind_addr = "0.0.0.0:8080"

[queue]
workers = 8
rate_limit_per_worker = 10
max_retries = 3

[ledger]
path = "data/ledger"
retention_days_jobs = 30

[dlq]
path = "data/dlq"
retention_days = 90

[storage]
backend = "s3"
bucket = "my-service-artifacts"

# Define your job types
[job_types.my-service]
handler = "fetchbox_handlers::resource::ResourceHandler"

[job_types.my-service.storage]
bucket = "my-service-artifacts"

[job_types.my-service.options]
max_parallel_downloads = 8
```

### 2. Submit Jobs

```python
import requests
import json

# Example Python integration
class FetchBoxClient:
    def __init__(self, base_url: str):
        self.base_url = base_url
    
    def submit_job(self, manifest: dict) -> str:
        """Submit a job and return job_id for tracking"""
        response = requests.post(
            f"{self.base_url}/jobs",
            headers={
                "Content-Type": "application/json",
                "X-Fetchbox-Job-Type": "my-service",
                "X-Fetchbox-Tenant": "my-tenant"
            },
            json=manifest
        )
        
        if response.status_code == 202:
            return response.json()["job_id"]
        else:
            raise Exception(f"Job submission failed: {response.text}")
    
    def get_job_status(self, job_id: str) -> dict:
        """Get current job status and progress"""
        response = requests.get(f"{self.base_url}/operators/jobs/{job_id}")
        return response.json()
```

### 3. Job Manifest Structure

```json
{
  "manifest_version": "v1",
  "storage": {
    "manifest_file": "metadata.json",
    "resource_key_prefix": "resources/2024/05/01/my-service/"
  },
  "metadata": {
    "source": "my-crawler",
    "batch_id": "batch-123",
    "custom_field": "custom_value"
  },
  "resources": [
    {
      "name": "image1.jpg",
      "url": "https://example.com/image1.jpg",
      "headers": {
        "User-Agent": "MyCrawler/1.0",
        "Referer": "https://example.com/page.html"
      },
      "tags": {
        "content_type": "image/jpeg",
        "priority": "high"
      }
    },
    {
      "name": "document.pdf",
      "url": "https://example.com/document.pdf",
      "headers": {},
      "tags": {
        "content_type": "application/pdf"
      }
    }
  ],
  "attributes": {
    "tenant": "my-service",
    "priority": "normal",
    "crawl_id": "847458834325543643"
  }
}
```

## Advanced Integration Patterns

### Custom Handlers

For specialized processing logic, implement custom handlers:

```rust
use async_trait::async_trait;
use fetchbox::handlers::{JobHandler, ManifestEnvelope, PreparedManifest, DownloadTask};

#[derive(Clone)]
struct MyCustomHandler {
    config: HandlerConfig,
}

#[async_trait]
impl JobHandler for MyCustomHandler {
    async fn prepare_manifest(
        &self,
        envelope: ManifestEnvelope,
    ) -> Result<PreparedManifest, HandlerError> {
        // Validate manifest, extract business context
        // Add custom metadata to handler_context
        Ok(PreparedManifest {
            envelope,
            handler_context: json!({ "custom_field": "value" }),
        })
    }

    async fn build_tasks(
        &self,
        prepared: PreparedManifest,
    ) -> Result<Vec<DownloadTask>, HandlerError> {
        // Transform resources into download tasks
        // Add custom headers, storage hints, proxy configuration
        let tasks = prepared.envelope
            .manifest
            .resources
            .into_iter()
            .map(|resource| {
                let mut task = DownloadTask::from_resource(
                    &resource,
                    &self.config.default_headers,
                    storage_hint,
                    proxy_hint,
                );
                
                // Add custom tags and attributes
                task.tags.insert("handler".to_string(), "custom".to_string());
                task.attributes = Some(json!({ "processed_by": "my-handler" }));
                
                task
            })
            .collect();
        
        Ok(tasks)
    }
}
```

Register custom handlers in configuration:

```toml
[job_types.my-custom]
handler = "my_service::handlers::MyCustomHandler"

[job_types.my-custom.storage]
bucket = "my-custom-artifacts"

[job_types.my-custom.options]
custom_option = "value"
```

### Error Handling and Retry Logic

FetchBox provides built-in retry mechanisms:

- **Automatic retries** - Configurable exponential backoff
- **Dead Letter Queue** - Failed tasks moved to DLQ after max retries
- **Error classification** - HTTP errors, network timeouts, storage failures

```python
# Check for failed resources
def check_job_failures(fetchbox_client, job_id):
    status = fetchbox_client.get_job_status(job_id)
    
    if status["resource_failed"] > 0:
        print(f"Job {job_id} has {status['resource_failed']} failed resources:")
        for error in status.get("errors", []):
            print(f"  - {error['resource_name']}: {error['code']} - {error['message']}")
    
    return status["resource_failed"]
```

### Storage Integration

FetchBox stores artifacts in S3-compatible storage using the paths specified in your manifest. Once jobs are completed, further storage usage and artifact management is the responsibility of your service.

Artifacts are stored at:
- **Manifest**: `s3://{bucket}/{storage.manifest_file}`
- **Resources**: `s3://{bucket}/{storage.resource_key_prefix}/{resource_name}`

Your service should implement its own logic for retrieving and processing these artifacts based on your specific requirements.

## Production Deployment

### Horizontal Scaling

Deploy multiple FetchBox instances behind a load balancer. Each instance maintains its own queue and state, making horizontal scaling straightforward.

### Health Monitoring

```python
# Health check integration
def monitor_fetchbox_health(fetchbox_client):
    try:
        response = requests.get(f"{fetchbox_client.base_url}/operators/health")
        health = response.json()
        
        if health["status"] == "healthy":
            print("FetchBox is healthy")
            for component, status in health["components"].items():
                print(f"  {component}: {status}")
        else:
            print("FetchBox is unhealthy")
            
    except Exception as e:
        print(f"Health check failed: {e}")
```

### Configuration Management

Use environment-specific configurations:

```toml
# config/development.toml
[storage]
backend = "memory"  # Use in-memory storage for development

# config/production.toml  
[storage]
backend = "s3"
bucket = "my-service-production"

[queue]
workers = 16  # Scale up for production
```

## Best Practices

### 1. Job Organization

- Use date-based storage prefixes for easy organization
- Include tenant information in manifests for multi-tenant systems
- Set appropriate priority levels for different job types

### 2. Error Handling

- Always check job status after submission
- Implement retry logic for transient failures
- Monitor DLQ for persistent failures

### 3. Performance Optimization

- Batch similar resources in single manifests
- Use appropriate worker counts for your workload
- Monitor queue depth and adjust scaling accordingly

### 4. Security

- Use HTTPS for all external URLs
- Validate and sanitize input URLs

## Troubleshooting

### Common Issues

1. **Job submission fails with 400**
   - Check manifest schema compliance
   - Verify required headers are present
   - Ensure resource URLs are valid

2. **Jobs stuck in queued state**
   - Check worker health and count
   - Verify storage backend connectivity
   - Monitor system resources

3. **High failure rate**
   - Check target URL availability
   - Review rate limiting settings
   - Inspect DLQ for error patterns

### Debugging Tools

```bash
# Check FetchBox health
curl http://localhost:8080/operators/health

# Get job status
curl http://localhost:8080/operators/jobs/{job_id}

# View logs
tail -f /var/log/fetchbox.log
```

## Next Steps

1. **Start Simple** - Begin with basic job submission and status checking
2. **Add Monitoring** - Implement health checks and alerting
3. **Scale Gradually** - Increase worker count as load grows
4. **Customize** - Implement custom handlers for specialized logic
5. **Optimize** - Fine-tune configuration based on usage patterns

For more detailed examples, see the `examples/` directory in the FetchBox repository.
