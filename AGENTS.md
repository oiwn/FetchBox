# Repository Guidelines

## Project Structure & Module Organization
- **Single binary, no workspaces**: The repository builds ONE CLI binary `fetchbox`; `src/main.rs` dispatches subcommands (`api`, `worker`, etc.) into modules under `src/`.
- **Use modules, not separate crates**: All functionality lives inside the main crate's module tree (`src/api`, `src/handlers`, `src/proto`, `src/config`, etc.). Do NOT create workspace crates like `crates/fetchbox_xyz` unless explicitly required.
- **Module structure examples**:
  - `src/handlers/` - Handler trait system
  - `src/proto/` - Generated protobuf types
  - `src/api/` - API services
  - `src/streams/` - Stream definitions
- `specs/` contains task-by-task specifications; treat each `specs/task_XX.md` as the contract before coding.
- `specs/current_task.md` points to current status; `specs/progress.md` tracks implementation history.
- `docs/` mirrors polished specs (`docs/requirements.md`, `docs/messaging.md`, etc.) for operators.
- `config/`, `docker-compose.dev.yml`, and `scripts/` host local environment assets when introduced.

## Current task

`specs/ctx.md` contains current task and it context. Keep it simple and updated to current state of things. Relevant information also should be stored there.

## Build, Test, and Development Commands
- `cargo check` — fast validation of the entire crate; run before opening a PR.
- `cargo fmt --all` — format codebase per `rustfmt.toml`.
- `cargo clippy --all-targets -- -D warnings` — lint with Clippy; fail on warnings.
- `cargo test --all` — execute unit tests; integration tests will live under `tests/`.
- `cargo add <crate_name>` — **ALWAYS use this to add dependencies**. It automatically fetches the latest compatible version instead of manually editing `Cargo.toml`.
- `make dev-up` / `make dev-down` (once added) — spin up/down Iggy + MinIO via Docker Compose for end-to-end testing.

## Coding Style & Naming Conventions
- Rust edition 2024, enforced via `rustfmt.toml`; prefer 4-space indentation and trailing commas in multi-line structures.
- Module names use `snake_case`; public types and traits use `CamelCase`.
- Feature flags default to off; enable via `--features`.
- Keep handler paths fully-qualified (e.g., `fetchbox_handlers::gallery::GalleryHandler`) to match config expectations.

## Testing Guidelines
- Unit tests live next to implementation files (`mod tests { ... }`).
- Integration tests belong in `tests/` and may use `testcontainers` to start Iggy + MinIO.
- Name tests with intent (`downloads_large_file`, `proxy_rotates_on_failure`).
- Ensure new features include coverage for happy path + failure handling (especially retries/DLQ logic).
- **If tests, compilation, or build fails**: STOP immediately. Describe the problem to the human and wait for instructions. Do NOT attempt to fix compilation errors by guessing APIs or making multiple attempts without user guidance.

## Planning & Implementation Guidelines
- **Do NOT provide time estimations** in implementation plans. Focus on steps, deliverables, and decision points only.
- When creating plans, structure them as: Prerequisites -> Steps -> Deliverables -> Decision Points.
- Keep plans actionable and focused on technical approach, not duration.

## Commit & Pull Request Guidelines
- Commit messages: short imperative subject (`Add handler registry`) plus optional body explaining reasoning.
- Reference related spec or issue (`Refs #task-02`) when applicable.
- PR checklist:
  1. Link the relevant spec (`specs/task_XX_*.md`) and summarize how acceptance criteria are met.
  2. Include testing evidence (`cargo test`, integration logs).
  3. Attach screenshots or logs for operator surfaces if UI/CLI output changed.
  4. Keep PRs focused on a single backlog item to ease review.

## Security & Configuration Tips
- Never commit real credentials; rely on `.env.example` templates.
- Validate config changes against `fetchbox_config` schema and document configuration as file-level comment..
