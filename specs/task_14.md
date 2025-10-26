# Spec: Documentation & Operator Runbook

## Goal
Consolidate FetchBox documentation for contributors and operators, including architecture overview, handler authoring guide, log consumption instructions, deployment checklist, and troubleshooting tips.

## Scope
1. Update `README.md` with concise overview and quickstart.
2. Create `docs/` structure:
   - `docs/architecture.md`
   - `docs/requirements.md` (from task_01)
   - `docs/configuration.md`
   - `docs/messaging.md`
   - `docs/ledger.md`
   - `docs/storage.md`
   - `docs/observability.md`
   - `docs/development.md`
   - `docs/testing.md`
   - `docs/runbook.md`
3. Provide handler authoring guide referencing `fetchbox_plugins`.
4. Document deployment steps and operator procedures.

## 1. README Updates
- Include:
  - Project summary.
  - High-level architecture diagram (ASCII or link to image).
  - Quickstart (link to dev environment doc).
  - Pointer to docs index.

## 2. Architecture Doc
- Describe components: Axum API, Iggy, Worker, Fjall, Storage.
- Include sequence diagrams for ingest and worker flows.
- Mention extension points (handlers, proxies).

## 3. Handler Authoring Guide
- Live in `docs/handlers.md`.
- Cover:
  - Trait overview.
  - Example handler (from sample crate).
  - Testing handlers.
  - Deploying custom handlers (rebuilding binary).

## 4. Runbook (`docs/runbook.md`)
- Sections:
  - **Monitoring**: metrics to watch (queue lag, DLQ, proxy failures).
  - **Operational Tasks**:
    - Rotate proxy credentials.
    - Update handler config.
    - Prune ledger manually.
    - Replay DLQ tasks.
  - **Incident Response**:
    - High DLQ rate.
    - Iggy backlog.
    - Storage errors.
  - **Scaling**:
    - Add worker instances.
    - Tune Iggy partitions.
  - **Security**:
    - Enabling auth once implemented (placeholder).

## 5. Log Consumption Guide
- Document how to tail logs via:
  - Operator API SSE endpoint.
  - `fetchbox-iggy-cli tail-logs`.
  - Direct Iggy consumer.

## 6. Deployment Checklist
- Add `docs/deployment.md` with:
  - Required services.
  - Config variables.
  - Rolling upgrade procedure.
  - Backup plan (Fjall snapshots, MinIO bucket retention).

## 7. Deliverables
- Updated README.
- New docs as listed above (some already produced; ensure cross-links).
- Ensure `mdbook` or simple TOC in `docs/README.md` linking to all files.
