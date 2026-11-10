"""Optional real Siteverify protocol probe using ONLY Cloudflare's public test keys."""
import json
from pathlib import Path
import urllib.error
import urllib.parse
import urllib.request

ENDPOINT = "https://challenges.cloudflare.com/turnstile/v0/siteverify"
CASES = {
    "test_pass": ("1x0000000000000000000000000000000AA", "XXXX.DUMMY.TOKEN.XXXX"),
    "test_reject": ("2x0000000000000000000000000000000AA", "XXXX.DUMMY.TOKEN.XXXX"),
    "test_duplicate": ("3x0000000000000000000000000000000AA", "XXXX.DUMMY.TOKEN.XXXX"),
    "empty_secret": ("", "XXXX.DUMMY.TOKEN.XXXX"),
    "invalid_secret": ("not-a-real-secret-fixture", "XXXX.DUMMY.TOKEN.XXXX"),
    "empty_response": ("1x0000000000000000000000000000000AA", ""),
}


def main():
    results = {}
    for name, (secret, token) in CASES.items():
        body = urllib.parse.urlencode({"secret": secret, "response": token}).encode()
        try:
            response = urllib.request.urlopen(ENDPOINT, body, timeout=15)
        except urllib.error.HTTPError as error:
            response = error
        results[name] = {"http_status": response.status,
                         "body": json.loads(response.read())}
    assert results["test_pass"]["body"]["success"] is True, results
    for name in CASES.keys() - {"test_pass"}:
        assert results[name]["body"]["success"] is False, results
    target = Path(__file__).resolve().parent / "artifacts" / "provider.json"
    target.parent.mkdir(exist_ok=True)
    target.write_text(json.dumps({"test_only": True, "endpoint": ENDPOINT,
                                  "results": results}, indent=2) + "\n")
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
