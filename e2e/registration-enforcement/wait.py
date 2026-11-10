"""Bounded startup wait only; health is NOT readiness/attestation evidence."""
import time
import urllib.error
import urllib.request

for attempt in range(90):
    try:
        with urllib.request.urlopen("http://127.0.0.1:19433/health/ready", timeout=2) as response:
            if response.status == 200:
                break
    except (OSError, urllib.error.URLError):
        pass
    time.sleep(1)
else:
    raise SystemExit("Dedicated Kratos failed to start")
