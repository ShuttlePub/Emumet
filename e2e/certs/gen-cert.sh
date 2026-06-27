#!/bin/sh
# Generate self-signed certificate for AP E2E testing
# All four nip.io domains share one cert via SAN

set -e

CERT_DIR="$(cd "$(dirname "$0")" && pwd)"

openssl req -x509 \
  -newkey rsa:4096 \
  -keyout "${CERT_DIR}/key.pem" \
  -out "${CERT_DIR}/cert.pem" \
  -days 365 \
  -nodes \
  -subj "/CN=emumet.127.0.0.1.nip.io" \
  -addext "subjectAltName=DNS:emumet.127.0.0.1.nip.io,DNS:peer.127.0.0.1.nip.io,DNS:iceshrimp.127.0.0.1.nip.io,DNS:mastodon.127.0.0.1.nip.io"

echo "Generated:"
echo "  ${CERT_DIR}/cert.pem"
echo "  ${CERT_DIR}/key.pem"
