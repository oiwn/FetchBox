# FetchBox dev commands

# Run end-to-end integration tests
test-e2e:
    @echo "Running end-to-end integration tests..."
    cargo test --test e2e -- --test-threads=1 --nocapture
    just iggy-stop

# Run all tests (unit + integration)
test-all: test-e2e
    @echo "Running unit tests..."
    cargo test --lib
    cargo test --test api_test

