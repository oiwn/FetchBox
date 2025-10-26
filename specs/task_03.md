# Spec: Messaging Schemas & Iggy Streams

## Goal
Define the Iggy stream layout, message schemas, and retention/consumption rules so Axum ingress, downloader workers, operator tooling, and optional direct producers all speak the same protocol. This spec results in concrete protobuf definitions plus documentation for stream creation and usage.

## Scope
1. Enumerate all required Iggy streams/topics (`jobs.tasks`, `jobs.status`, `jobs.logs`, `jobs.dlq`).
2. Define message schemas in protobuf (stored under `proto/jobs.proto`) for each stream.
3. Specify retention, partitioning, and consumer group conventions.
4. Document publishing/consuming responsibilities for each service (Axum API, worker, operators, direct producers).
5. Provide guidance for direct producers so they can publish compliant messages.

## 1. Streams Overview

| Stream        | Purpose                                         | Producer(s)       | Consumer(s)                | Retention | Partitions |
|---------------|-------------------------------------------------|-------------------|----------------------------|-----------|------------|
| `jobs.tasks`  | Per-resource download tasks                     | Axum API, direct clients | Downloader workers         | 7 days    | 8          |
| `jobs.status` | Job-level status updates (queued, running, done)| Workers            | Operator services, Fjall mirror | 14 days   | 4          |
| `jobs.logs`   | Structured log envelopes per job/task           | Workers            | Operator log tailer, observability stack | 30 days   | 4          |
| `jobs.dlq`    | Permanently failed tasks after retries          | Workers            | Operators, replay tooling  | 90 days   | 2          |

- Streams live under the `fetchbox` namespace in Iggy (`fetchbox.jobs.tasks`, etc.).
- All streams use at-least-once semantics; consumers track offsets via configured consumer groups:
  - API uses producer-only access (no group).
  - Workers consume `jobs.tasks` using consumer group `workers`.
  - Operator services consume `jobs.status`/`jobs.logs` using group `operators`.

## 2. Message Schemas (Protobuf)

File: `proto/jobs.proto`

```proto
syntax = "proto3";
package fetchbox.jobs;

message HttpHeader {
  string name = 1;
  string value = 2;
}

message StorageHint {
  string bucket = 1;
  string key_prefix = 2;
  map<string, string> metadata = 3;
}

message ProxyHint {
  string primary_pool = 1;
  repeated string fallbacks = 2;
}

message TaskAttributes {
  map<string, string> tags = 1;
  string checksum_hint = 2;
  string mime_hint = 3;
  bytes extra = 4; // optional serialized blob
}

message DownloadTask {
  string job_id = 1;
  string job_type = 2;
  string resource_id = 3;
  string url = 4;
  repeated HttpHeader headers = 5;
  ProxyHint proxy_hint = 6;
  StorageHint storage_hint = 7;
  TaskAttributes attributes = 8;
  string manifest_key = 9;
  uint32 attempt = 10;
  string tenant = 11;
  string trace_id = 12;
}

enum JobState {
  JOB_STATE_UNKNOWN = 0;
  JOB_STATE_QUEUED = 1;
  JOB_STATE_IN_PROGRESS = 2;
  JOB_STATE_PARTIAL = 3;
  JOB_STATE_COMPLETED = 4;
  JOB_STATE_FAILED = 5;
}

message JobStatus {
  string job_id = 1;
  string job_type = 2;
  JobState state = 3;
  uint32 resources_total = 4;
  uint32 resources_completed = 5;
  uint32 resources_failed = 6;
  string manifest_key = 7;
  string tenant = 8;
  string last_error_code = 9;
  string last_error_message = 10;
  uint64 log_offset = 11;
  uint64 timestamp_ms = 12;
}

enum LogLevel {
  LOG_LEVEL_TRACE = 0;
  LOG_LEVEL_DEBUG = 1;
  LOG_LEVEL_INFO = 2;
  LOG_LEVEL_WARN = 3;
  LOG_LEVEL_ERROR = 4;
}

message JobLog {
  string job_id = 1;
  string resource_id = 2;
  LogLevel level = 3;
  string message = 4;
  map<string, string> fields = 5;
  uint64 timestamp_ms = 6;
  string trace_id = 7;
}

message DeadLetterTask {
  DownloadTask task = 1;
  string failure_code = 2;
  string failure_message = 3;
  uint32 attempts = 4;
  uint64 failed_at_ms = 5;
}
```

Requirements:
- All fields are required unless documented optional (protobuf handles defaults).
- Headers should be canonicalized (lowercase names) before publishing.
- `trace_id` follows W3C trace-context (16/32 hex chars).
- JSON representation (for debugging/direct producers) can be obtained via serde when necessary.

## 3. Stream Provisioning & Retention
- Provide migration script (`scripts/setup_iggy_streams.rs` or CLI commands) that creates streams/partitions with retention policies listed above.
- Retention measured in time and size (time dominates). Example: 7 days or 100 GB for `jobs.tasks`.
- DLQ stream uses 90-day retention; manual clean-up via operator command.

## 4. Publishing & Consumption Rules
- Axum API:
  - Publishes `DownloadTask` messages to `jobs.tasks` (partitioning by `job_id` hash for affinity).
  - Emits initial `JobStatus` (`QUEUED`) after manifest write (either via same process or by publishing to `jobs.status`).
- Downloader worker:
  - Consumes `jobs.tasks` via consumer group `workers`.
  - After each task completes/fails, publishes `JobStatus` updates (aggregated per job) and `JobLog` entries for observability.
  - On terminal failure, publishes `DeadLetterTask` to `jobs.dlq`.
- Operator service:
  - Consumes `jobs.status` to update Fjall snapshots.
  - Offers log tailing by replaying `jobs.logs` from stored offsets.
- Direct producers:
  - Must serialize `DownloadTask` using the protobuf schema and publish to `jobs.tasks`.
  - Optional: publish synthetic `JobStatus` entries if they replace Axum entirely; otherwise rely on core service.

## 5. Tooling & Docs
- Generate Rust types via `prost-build` into a new crate/module (e.g., `crates/fetchbox_proto`).
- Document in `docs/messaging.md`:
  - Stream purposes.
  - How to run the provisioning script.
  - Examples of encoding/decoding `DownloadTask`.
  - Guidelines for direct producers (including CLI snippet using `prost`).

## 6. Deliverables
- `proto/jobs.proto` with schemas above.
- Build script to generate Rust bindings (`build.rs` or dedicated crate).
- Stream provisioning script/commands.
- `docs/messaging.md` describing streams, schemas, and producer/consumer responsibilities.
