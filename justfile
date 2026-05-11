# Start docker compose services
up:
    docker compose up -d

# Stop all services
down:
    docker compose down -v

# Wait for all services to become healthy
wait-services:
    @echo "Waiting for services to be healthy..."
    @n=0; until docker compose exec -T postgres pg_isready -U postgres > /dev/null 2>&1; do \
        n=$((n+1)); if [ $n -ge 30 ]; then echo "ERROR: postgres not ready after 60s"; exit 1; fi; \
        echo "Waiting for postgres..."; sleep 2; \
    done
    @n=0; until docker compose exec -T redis redis-cli ping 2>/dev/null | grep -q PONG; do \
        n=$((n+1)); if [ $n -ge 30 ]; then echo "ERROR: redis not ready after 60s"; exit 1; fi; \
        echo "Waiting for redis..."; sleep 2; \
    done
    @for port in 4433 4444 4445 4466 4467; do \
        n=0; until curl -sf http://localhost:$port/health/alive > /dev/null 2>&1; do \
            n=$((n+1)); if [ $n -ge 30 ]; then echo "ERROR: port $port not ready after 60s"; exit 1; fi; \
            echo "Waiting for port $port..."; sleep 2; \
        done; \
    done
    @echo "All services healthy."

# Verify master-key-password file exists
check-master-key:
    @test -f master-key-password || (echo "ERROR: master-key-password file not found" && exit 1)

# Run database migrations (requires sqlx-cli)
migrate:
    sqlx migrate run

# Run E2E tests only (services and prerequisites must be ready)
e2e-test:
    cargo build -p server
    cargo test -p server --test e2e_basic_flow -- --ignored --test-threads=1 --nocapture

# Full local E2E flow: start services, wait, verify, run tests
e2e: up wait-services check-master-key
    @just migrate
    @just e2e-test

# Alias for stopping services
e2e-down: down
