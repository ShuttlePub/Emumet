# Run E2E tests (requires docker compose services running)
e2e:
    docker compose up -d
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
    @test -f master-key-password || (echo "ERROR: master-key-password file not found" && exit 1)
    @echo "All services healthy. Running E2E tests..."
    cargo test -p server --test e2e_basic_flow -- --ignored --test-threads=1
    @echo "E2E tests complete."

# Stop all services
e2e-down:
    docker compose down -v
