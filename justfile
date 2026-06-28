# Run full E2E test suite (AP ActivityPub tests)
e2e:
    bash e2e/run-ap-e2e.sh

# Run basic E2E tests only (fast, no AP federation)
e2e-basic:
    cargo build -p server
    cargo test -p server --test e2e_basic_flow -- --ignored --test-threads=1 --nocapture
