# FetchBox

FetchBox is a single-process download orchestration service: an Axum HTTP API accepts manifests, embedded workers fan tasks out through bounded channels, and results are persisted to Fjall + S3-compatible storage. The whole stack ships as one Rust binary, so you can run it locally, embed it, or ship it to production without extra brokers.

## Highlights
- **Single binary runtime** – Axum API, TaskBroker, Fjall queue/DLQ, and worker pool live in one process.
- **Deterministic storage** – UUIDv7 sequencing, Fjall-backed queue/ledger, and DLQ snapshots keep every task recoverable.
- **S3-compatible pipeline** – The default storage layer targets AWS S3/MinIO via `object_store` with handler-level overrides.
- **Extensible handlers** – Custom handler registries convert manifests into per-resource download tasks, add headers/proxies, and control storage metadata.
- **Operator visibility** – Structured logs, ready endpoints, and DLQ artifacts make it easy to inspect job state or replay failures.

## Getting Started

### Prerequisites
- Rust toolchain **1.90+** (install via [`rustup`](https://rustup.rs/)).
- `cargo` available on `PATH`.
- Access to an S3-compatible bucket (the examples target MinIO).
- Optional: [`mc`](https://min.io/docs/minio/linux/reference/minio-mc.html) CLI for inspecting MinIO buckets.

### Install & Run
1. Clone the repository:
   ```bash
   git clone https://github.com/<org>/FetchBox.git
   cd FetchBox
   ```
2. Copy the sample config and customize as needed:
   ```bash
   cp config/fetchbox.example.toml config/fetchbox.toml
   ```
3. Export storage credentials (MinIO or AWS). Either `S3_*` or AWS-style env vars work:
   ```bash
   export S3_ACCESS_KEY=minioadmin
   export S3_SECRET_KEY=minioadmin
   export AWS_REGION=us-east-1              # optional
   ```
   You can also create a `.env` file (see `.env.example`) and rely on `dotenvy`.
4. Run the API/worker binary with your config:
   ```bash
   cargo run --bin fetchbox -- --config config/fetchbox.toml
   ```
   The server binds to `bind_addr` from the config (default `0.0.0.0:8080`) and immediately starts accepting `POST /jobs` manifests.

### Configuration Quick Reference
- `config/fetchbox.example.toml` enumerates every knob (queue paths, worker counts, handler registry, proxy pools, retention policies).
- Environment overrides follow the `FETCHBOX__SECTION__KEY=value` pattern (e.g., `FETCHBOX__QUEUE__WORKERS=8`).
- Storage credentials are never stored in the TOML file—use env vars (`S3_ACCESS_KEY`, `AWS_ACCESS_KEY_ID`, etc.) or a `.env`.
- The MinIO walkthrough uses `config/httpbin.minio.toml` (template values are replaced at runtime).

## Examples

### httpbin + MinIO End-to-End Walkthrough
This example exercises the full data path (manifest ingest → download → S3 upload → DLQ) using local MinIO storage.

1. **Start MinIO**  
   ```bash
   minio server tmp/data/
   ```
2. **Bootstrap the bucket (one time)**  
   ```bash
   mc alias set local http://127.0.0.1:9000 minioadmin minioadmin
   mc mb local/fetchbox-httpbin || true
   ```
3. **Export example credentials (or add them to `.env`)**  
   ```bash
   export S3_ACCESS_KEY=minioadmin
   export S3_SECRET_KEY=minioadmin
   export AWS_REGION=us-east-1
   ```
4. **Run the walkthrough**  
   ```bash
   cargo run --example httpbin_minio
   ```
   The example renders a temporary config from `config/httpbin.minio.toml`, rewrites manifest storage so manifests and resources share `s3://fetchbox-httpbin/YYYY/MM/DD/httpbin_minio/`, launches the API, and blocks until all tasks finish.
5. **Inspect the bucket output**  
   ```bash
   mc ls local/fetchbox-httpbin
   mc ls local/fetchbox-httpbin/<prefix>/
   mc cat local/fetchbox-httpbin/<prefix>/anything.json   # successful resource
   mc cat local/fetchbox-httpbin/<prefix>/timeout.json    # intentional timeout artifact (DLQ)
   ```

Every example lives under `examples/`; follow the instructions in each README/comment block to run them.

## Development & Testing

Before opening a PR or cutting a release, run the complete workflow:

```bash
cargo fmt --all
cargo check
cargo test --all
cargo clippy --all-targets -- -D warnings
just test-e2e        # optional integration pass, requires fixtures
```

Additional notes:
- Keep `specs/ctx.md` up to date with the active plan/checklist.
- Update `specs/overview.md` whenever architecture status or spec ownership changes.
- New features should include unit tests adjacent to the implementation and, when applicable, integration tests under `tests/`.

## Contribution Workflow
1. Read `AGENTS.md` for repository-wide expectations (planning, security, tooling).
2. Create a branch and update `specs/ctx.md` with the task plan/checklist you are executing.
3. Implement changes following the coding/test guidelines above.
4. Update relevant docs (`README.md`, `specs/overview.md`, config examples) whenever behavior or workflows change.
5. Ensure `cargo fmt` + `cargo clippy` + `cargo test --all` complete successfully before requesting review.

## Release Readiness Checklist
- ✅ `cargo fmt --all`, `cargo check`, `cargo test --all`, and `cargo clippy --all-targets -- -D warnings`.
- ✅ `README.md`, `AGENTS.md`, `specs/overview.md`, and `specs/ctx.md` are in sync with the current release plan.
- ✅ Deprecated specs removed; `specs/` only contains authoritative contracts.
- ✅ `examples/httpbin_minio` (or equivalent) runs end-to-end with the documented steps.
- ✅ Repository-wide secrets sweep performed (search for keys/tokens, confirm `.gitignore`/`.env.example` cover sensitive files).

## License

Licensed under the terms specified in [`LICENSE`](LICENSE).
