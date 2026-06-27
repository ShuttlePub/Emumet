#!/usr/bin/env bash
set -euo pipefail

# Check Mastodon health via nginx proxy.
#
# Prerequisites:
#   - Docker compose with --profile ap-e2e must be running
#   - python3 is required for JSON pretty-printing
#   - curl is required for HTTP requests
#
# Usage:
#   bash e2e/check-mastodon.sh

MASTODON_URL="https://mastodon.127.0.0.1.nip.io:8443/api/v1/instance"

echo "Checking Mastodon..."

for i in $(seq 1 30); do
    response=$(curl -sk -o /dev/null -w "%{http_code}" \
        "$MASTODON_URL" 2>/dev/null || true)

    if [ "$response" = "200" ]; then
        echo "Mastodon is healthy! (attempt $i)"
        curl -sk "$MASTODON_URL" | \
            python3 -c "
import sys, json
d = json.load(sys.stdin)
print(f'  Instance: {d.get(\"uri\", \"?\")}')
print(f'  Title:    {d.get(\"title\", \"?\")}')
print(f'  Version:  {d.get(\"version\", \"?\")}')
" 2>/dev/null || echo "  (could not parse JSON response)"
        exit 0
    fi

    echo "  mastodon not ready yet (HTTP $response), attempt $i/30..."
    sleep 5
done

echo "Mastodon failed to become healthy within 150s"
exit 1
