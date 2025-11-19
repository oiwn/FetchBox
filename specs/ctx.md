# Context: Current tasks

Pending definition — populate this section once the next task is scoped._

## Potential next tasks
1. **Embeddable handler registry** – Refactor runtime/bootstrap so embedders can supply a custom `HandlerRegistry` (instead of always calling `with_defaults()`), document the public API, and add tests/examples demonstrating customization.
2. **Handler-provided worker hooks** – Design a lightweight mechanism for handlers to influence worker behavior (e.g., custom download pipeline, storage metadata) without forking the runtime.
3. **Operator-facing API surface** – Expand `/operators/*` endpoints (or CLI tooling) to expose DLQ listings, job logs, and storage pointers now that the S3 pipeline is validated.
