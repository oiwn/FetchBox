# Repository Guidelines

## Project Structure & Module Organization
- Root-level packages (`fetchbox_api/`, `fetchbox_worker/`, `fetchbox_plugins/`, etc.) participate in the top-level Cargo workspace (see `Cargo.toml` `[workspace]` members).
- `src/` currently holds placeholder code; future shared libraries can live at root as well (`fetchbox_storage/`, `fetchbox_ledger/`).
- `specs/` contains task-by-task specifications; treat each `specs/task_XX.md` as the contract before coding.
- `specs/current_task.md` records the active architecture context and backlog; update it as we refine requirements.
- `docs/` mirrors polished specs (`docs/requirements.md`, `docs/messaging.md`, etc.) for operators.
- `config/`, `docker-compose.dev.yml`, and `scripts/` host local environment assets when introduced.

## Build, Test, and Development Commands
- `cargo check` — fast validation of the entire workspace; run before opening a PR.
- `cargo fmt --all` — format codebase per `rustfmt.toml`.
- `cargo clippy --all-targets -- -D warnings` — lint with Clippy; fail on warnings.
- `cargo test --all` — execute unit tests; integration tests will live under `tests/`.
- `make dev-up` / `make dev-down` (once added) — spin up/down Iggy + MinIO via Docker Compose for end-to-end testing.

## Coding Style & Naming Conventions
- Rust edition 2021, enforced via `rustfmt.toml`; prefer 4-space indentation and trailing commas in multi-line structures.
- Module names use `snake_case`; public types and traits use `CamelCase`.
- Feature flags default to off; enable via `--features`.
- Keep handler paths fully-qualified (e.g., `fetchbox_handlers::gallery::GalleryHandler`) to match config expectations.

## Testing Guidelines
- Unit tests live next to implementation files (`mod tests { ... }`).
- Integration tests belong in `tests/` and may use `testcontainers` to start Iggy + MinIO.
- Name tests with intent (`downloads_large_file`, `proxy_rotates_on_failure`).
- Ensure new features include coverage for happy path + failure handling (especially retries/DLQ logic).

## Commit & Pull Request Guidelines
- Commit messages: short imperative subject (`Add handler registry`) plus optional body explaining reasoning.
- Reference related spec or issue (`Refs #task-02`) when applicable.
- PR checklist:
  1. Link the relevant spec (`specs/task_XX.md`) and summarize how acceptance criteria are met.
  2. Include testing evidence (`cargo test`, integration logs).
  3. Attach screenshots or logs for operator surfaces if UI/CLI output changed.
  4. Keep PRs focused on a single backlog item to ease review.

## Security & Configuration Tips
- Never commit real S3/Iggy credentials; rely on `.env.example` templates.
- Validate config changes against `fetchbox_config` schema and document any new environment variables in `docs/configuration.md`.
