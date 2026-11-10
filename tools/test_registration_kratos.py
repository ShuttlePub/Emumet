import copy
import json
import re
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from render_registration_kratos import PLACEHOLDER, TEMPLATE, render, validate_contract


class RegistrationKratosConfigTests(unittest.TestCase):
    def setUp(self):
        self.config = json.loads(TEMPLATE.read_text())

    def test_integration_migrate_and_serve_use_the_same_verified_image_digest(self):
        root = TEMPLATE.parents[2]
        digest = "sha256:c47903ba278bbcb330b5af4ce43d33dc17adacc1d40456b8e2a235add8ababa5"
        expected = "docker.io/oryd/kratos:v1.3.1@" + digest
        compose = (root / "e2e/registration-enforcement/compose.yml").read_text()
        images = re.findall(r"^\s+image:\s*(\S+)", compose, re.MULTILINE)
        self.assertEqual([ref for ref in images if "oryd/kratos" in ref], [expected, expected])
        baseline = (root / "e2e/registration-guard/evidence/manifest.json").read_text()
        self.assertIn("docker.io/oryd/kratos@" + digest, baseline)

    def test_all_enabled_registration_methods_have_authenticated_pre_persist_coverage(self):
        hook = validate_contract(self.config)
        self.assertEqual(hook["auth"]["config"]["name"], "Authorization")
        self.assertEqual(hook["response"], {"parse": True, "ignore": False})

    def test_adding_any_uncovered_registration_method_is_rejected(self):
        for method in ["oidc", "passkey", "webauthn", "unknown"]:
            with self.subTest(method=method):
                config = copy.deepcopy(self.config)
                config["selfservice"]["methods"][method] = {"enabled": True}
                with self.assertRaises(ValueError):
                    validate_contract(config)
        self.config["selfservice"]["methods"]["code"]["passwordless_enabled"] = True
        with self.assertRaises(ValueError):
            validate_contract(self.config)

    def test_parse_false_ignore_true_missing_or_global_only_hook_are_rejected(self):
        for key in ["parse", "ignore"]:
            config = copy.deepcopy(self.config)
            response = config["selfservice"]["flows"]["registration"]["after"]["password"]["hooks"][0]["config"]["response"]
            response[key] = not response[key]
            with self.assertRaises(ValueError):
                validate_contract(config)
        after = self.config["selfservice"]["flows"]["registration"]["after"]
        after["hooks"] = after.pop("password")["hooks"]
        with self.assertRaises(KeyError):
            validate_contract(self.config)

    def test_secret_is_rendered_only_in_copy_and_transport_is_restricted(self):
        key = "test-only-" * 6
        for url in ["http://127.0.0.1:8081/registration/pre-persist", "https://guard.internal/registration/pre-persist"]:
            config = render(self.config, key, url)
            hook = config["selfservice"]["flows"]["registration"]["after"]["password"]["hooks"][0]["config"]
            self.assertEqual(hook["auth"]["config"]["value"], "Bearer " + key)
            self.assertEqual(hook["url"], url)
        self.assertEqual(validate_contract(self.config)["auth"]["config"]["value"], PLACEHOLDER)
        for url in ["http://guard.internal/registration/pre-persist", "http://localhost/registration/pre-persist",
                    "https://user:password@guard.internal/registration/pre-persist",
                    "https://guard.internal/registration/pre-persist?key=secret", "file:///registration/pre-persist"]:
            with self.subTest(url=url), self.assertRaises(ValueError):
                render(self.config, key, url)
        for key in ["short", "with a space" * 10, "a" * 40 + "\n", "é" * 40]:
            with self.subTest(key_type="invalid"), self.assertRaises(ValueError):
                render(self.config, key, "http://127.0.0.1:8081/registration/pre-persist")

    def test_runtime_output_is_private_never_overwritten_and_key_is_not_logged(self):
        with tempfile.TemporaryDirectory() as temp:
            key = "unlogged-test-only-secret-" * 3
            secret = Path(temp) / "secret"
            output = Path(temp) / "runtime.json"
            secret.write_text(key)
            secret.chmod(0o600)
            command = [sys.executable, str(TEMPLATE.parents[2] / "tools/render_registration_kratos.py"),
                       "--secret-file", str(secret), "--output", str(output)]
            result = subprocess.run(command, capture_output=True, text=True, check=True)
            self.assertNotIn(key, result.stdout + result.stderr)
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)
            initial = output.read_bytes()
            result = subprocess.run(command, capture_output=True, text=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn(key, result.stdout + result.stderr)
            self.assertEqual(output.read_bytes(), initial)


if __name__ == "__main__":
    unittest.main()
