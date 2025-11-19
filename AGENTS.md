# Repository Guidelines

## Structure & Specs
- The crate builds a single CLI binary; all functionality lives in the main module tree under `src/`. Architecture boundaries and module ownership are documented in `specs/overview.md`.
- `specs/` is authoritative: each `specs/task_XX.md` is a contract, while `specs/ctx.md` captures the in-flight task context and must stay current with active work.
- Add a checkbox-based TODO section to `specs/ctx.md` to track current task progress explicitly.
- `config/` houses example configuration files. Avoid scattering environment assets elsewhere unless the plan explicitly calls for it.

## Release preparation requirements
- Keep documentation aligned with release expectations: refresh `AGENTS.md`, `README.md`, and `specs/overview.md` whenever the plan in `specs/ctx.md` changes.
- Resolve every `cargo clippy --all-targets -- -D warnings` finding, then rerun to prove a clean lint pass before merging or tagging a release.
- Clean up the specifications directory: remove deprecated specs, ensure surviving docs match the planned features, and avoid stale cross-references.
- Capture release steps (install, examples, contribution workflow) in `README.md` so GitHub visitors can reproduce the setup without tribal knowledge.
- Perform a secrets sweep prior to publishing: search for API keys, tokens, or credentials and verify `.gitignore`/`.env.example` are sufficient to prevent leaks.

## Build, Test, and Development Commands
- `cargo check` — fast validation of the entire crate; run before opening a PR.
- `cargo fmt --all` — format codebase per `rustfmt.toml`.
- `cargo clippy --all-targets -- -D warnings` — lint with Clippy; fail on warnings.
- `cargo test --all` — execute unit tests; integration tests will live under `tests/`.
- `cargo add <crate_name>` — **ALWAYS use this to add dependencies**. It automatically fetches the latest compatible version instead of manually editing `Cargo.toml`.

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

## Security & Configuration Tips
- Never commit real credentials; rely on `.env.example` templates.
- Validate config changes against `fetchbox_config` schema and document configuration as file-level comment.
