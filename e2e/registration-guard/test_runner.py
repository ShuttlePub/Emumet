"""Offline checks for evidence handling; not substitutes for real Kratos tests."""
import hashlib
import json
from pathlib import Path
import unittest

from run import mail_code, redact


class EvidenceTests(unittest.TestCase):
    def test_committed_evidence_matches_manifest(self):
        root = Path(__file__).resolve().parent
        manifest = json.loads((root / "evidence/manifest.json").read_text())
        self.assertEqual(manifest["full_suite_exit_code"], 0)
        for name, expected in manifest["sha256"].items():
            with self.subTest(file=name):
                self.assertEqual(hashlib.sha256((root / name).read_bytes()).hexdigest(),
                                 expected)

    def test_observed_binary_is_exact_version_not_similar_prefix(self):
        root = Path(__file__).resolve().parent
        version = json.loads((root / "evidence/version.json").read_text())["output"]
        self.assertRegex(version, r"Version:\s+v1\.3\.1(?:\s|$)")

    def test_initial_fixture_uses_method_specific_pre_persist_hook(self):
        root = Path(__file__).resolve().parent
        config = json.loads((root / "kratos.json").read_text())["selfservice"]
        hooks = config["flows"]["registration"]["after"]["password"]["hooks"]
        guard = next(h for h in hooks if h["hook"] == "web_hook")
        self.assertEqual(guard["config"]["response"], {"parse": True, "ignore": False})
        self.assertNotIn("after", config["flows"]["login"])
        self.assertFalse(config["methods"]["code"]["passwordless_enabled"])
        self.assertFalse(config["methods"]["password"]["config"]["haveibeenpwned_enabled"])

    def test_nested_credentials_and_flow_csrf_are_not_retained(self):
        original = {
            "session_token": "secret-session",
            "identity": {"credentials": {"password": "secret-password"}},
            "ui": {"nodes": [{"attributes": {"name": "csrf_token", "value": "secret-csrf"}}]},
        }
        result = redact(original)
        self.assertNotIn("secret-", str(result))
        self.assertEqual(original["session_token"], "secret-session")

    def test_synthetic_observations_are_preserved(self):
        evidence = {"before": {"db": []}, "identity_created": False,
                    "hook_calls": [{"email": "deny-api@example.test",
                                    "transient_payload": {"bot_token": "fixture-token"}}]}
        self.assertEqual(redact(evidence), evidence)

    def test_code_parser_decodes_plaintext_part_not_html_colors(self):
        raw = (
            'MIME-Version: 1.0\nContent-Type: multipart/alternative; boundary="fixture"\n\n'
            '--fixture\nContent-Type: text/plain; charset=UTF-8\n'
            'Content-Transfer-Encoding: quoted-printable\n\nYour code:\n123456\n'
            '--fixture\nContent-Type: text/html\n\n<p color="#000000">123456</p>\n'
            '--fixture--\n'
        )
        self.assertEqual(mail_code(raw), "123456")


if __name__ == "__main__":
    unittest.main()
