import hashlib
import json
import subprocess
import unittest

from loader import load

pr = load("publish-release.py", "publish_release")

REV = "c" * 40
OTHER = "d" * 40
LAYERS = ["sha256:" + "1" * 64, "sha256:" + "2" * 64]


def digest(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def make_image(revision=REV, arch="amd64", layers=LAYERS, media=pr.MANIFEST_MEDIA_TYPES[0]):
    config = json.dumps({
        "os": "linux", "architecture": arch,
        "config": {"Labels": {pr.REVISION_LABEL: revision}},
        "rootfs": {"type": "layers", "diff_ids": layers},
    }).encode()
    manifest = json.dumps({
        "schemaVersion": 2, "mediaType": media,
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": digest(config), "size": len(config)},
        "layers": [],
    }).encode()
    return manifest, config


def err(code, status):
    return pr.Response(status, {}, json.dumps({"errors": [{"code": code, "message": "m"}]}).encode())


class FakeRegistry:
    """A transport that behaves like an OCI registry with the Bearer token flow."""

    def __init__(self, manifest=None, config=None, manifest_status=None, require_auth=True, token_status=200):
        self.manifest, self.config = manifest, config
        self.manifest_status = manifest_status  # forces (status, Response) for the manifest GET
        self.require_auth = require_auth
        self.token_status = token_status
        self.requests = []

    def __call__(self, method, url, headers):
        self.requests.append((method, url, dict(headers)))
        if "/token" in url:
            if self.token_status != 200:
                return err("DENIED", self.token_status)
            return pr.Response(200, {}, b'{"token": "tok"}')
        if self.require_auth and headers.get("Authorization") != "Bearer tok":
            return pr.Response(401, {"www-authenticate": 'Bearer realm="https://reg.test/token",service="reg.test"'},
                               json.dumps({"errors": [{"code": "UNAUTHORIZED"}]}).encode())
        if "/manifests/" in url:
            if self.manifest_status is not None:
                return self.manifest_status
            if self.manifest is None:
                return err("MANIFEST_UNKNOWN", 404)
            return pr.Response(200, {"docker-content-digest": digest(self.manifest)}, self.manifest)
        if "/blobs/" in url:
            return pr.Response(200, {}, self.config)
        raise AssertionError(url)


def registry(transport, **kwargs):
    logs = []
    reg = pr.Registry("reg.test", "shuttlepub/emumet", transport=transport, log=logs.append, **kwargs)
    reg.logs = logs
    return reg


class Classify(unittest.TestCase):
    def test_absent_requires_unknown_code(self):
        reg = registry(FakeRegistry())
        self.assertEqual(reg.classify("2026.11.17", REV).state, "absent")
        self.assertTrue(any("HTTP 404" in line and "MANIFEST_UNKNOWN" in line for line in reg.logs))

    def test_name_unknown_is_absent(self):
        t = FakeRegistry(manifest_status=err("NAME_UNKNOWN", 404))
        self.assertEqual(registry(t).classify("v", REV).state, "absent")

    def test_bare_404_is_not_absent(self):
        t = FakeRegistry(manifest_status=pr.Response(404, {}, b"not found"))
        with self.assertRaises(pr.Stop):
            registry(t).classify("v", REV)

    def test_denied_unauthorized_5xx_are_stop(self):
        for response in (err("DENIED", 403), err("UNAUTHORIZED", 401), pr.Response(500, {}, b""), err("TOOMANYREQUESTS", 429)):
            with self.subTest(status=response.status):
                t = FakeRegistry(manifest_status=response, require_auth=False)
                with self.assertRaises(pr.Stop):
                    registry(t).classify("v", REV)

    def test_token_denied_is_undeterminable(self):
        t = FakeRegistry(token_status=403)
        reg = registry(t)
        with self.assertRaisesRegex(pr.Stop, "cannot tell"):
            reg.classify("v", REV)
        self.assertTrue(any("token request -> HTTP 403" in line and "DENIED" in line for line in reg.logs))

    def test_network_error_is_stop(self):
        def boom(method, url, headers):
            raise pr.TransportError("connection refused")

        with self.assertRaises(pr.Stop):
            registry(boom).classify("v", REV)

    def test_same_revision(self):
        manifest, config = make_image()
        state = registry(FakeRegistry(manifest, config)).classify("v", REV)
        self.assertEqual((state.state, state.digest, state.diff_ids), ("same", digest(manifest), LAYERS))

    def test_other_revision_stops(self):
        manifest, config = make_image(revision=OTHER)
        with self.assertRaisesRegex(pr.Stop, "refusing to overwrite"):
            registry(FakeRegistry(manifest, config)).classify("v", REV)

    def test_other_platform_stops(self):
        manifest, config = make_image(arch="arm64")
        with self.assertRaisesRegex(pr.Stop, "linux/arm64"):
            registry(FakeRegistry(manifest, config)).classify("v", REV)

    def test_image_index_stops(self):
        index = json.dumps({"schemaVersion": 2, "mediaType": pr.INDEX_MEDIA_TYPES[0], "manifests": []}).encode()
        with self.assertRaisesRegex(pr.Stop, "image index"):
            registry(FakeRegistry(index, b"")).classify("v", REV)

    def test_manifest_digest_header_mismatch_stops(self):
        manifest, config = make_image()
        t = FakeRegistry(manifest_status=pr.Response(200, {"docker-content-digest": "sha256:" + "0" * 64}, manifest))
        with self.assertRaisesRegex(pr.Stop, "digest mismatch"):
            registry(t).classify("v", REV)

    def test_tampered_config_stops(self):
        manifest, config = make_image()
        with self.assertRaisesRegex(pr.Stop, "does not match its digest"):
            registry(FakeRegistry(manifest, config + b" ")).classify("v", REV)

    def test_garbage_manifest_stops(self):
        t = FakeRegistry(manifest_status=pr.Response(200, {}, b"<html>"))
        with self.assertRaises(pr.Stop):
            registry(t).classify("v", REV)

    def test_redirected_blob_gets_no_credentials(self):
        manifest, config = make_image()
        calls = []

        def transport(method, url, headers):
            calls.append((url, dict(headers)))
            if "/manifests/" in url:
                return pr.Response(200, {"docker-content-digest": digest(manifest)}, manifest)
            if "/blobs/" in url:
                return pr.Response(307, {"location": "https://objects.test/blob?sig=1"}, b"")
            if url.startswith("https://objects.test/"):
                return pr.Response(200, {}, config)
            raise AssertionError(url)

        reg = registry(transport)
        reg._token = "tok"
        self.assertEqual(reg.classify("v", REV).state, "same")
        redirected = [h for u, h in calls if u.startswith("https://objects.test/")]
        self.assertEqual(redirected, [{}])


class Runner:
    def __init__(self, label=REV, layers=LAYERS, push_rc=0, on_push=None):
        self.label, self.layers, self.push_rc, self.on_push = label, layers, push_rc, on_push
        self.commands = []

    def __call__(self, command):
        self.commands.append(list(command))
        if command[:3] == ["docker", "image", "inspect"]:
            out = json.dumps({pr.REVISION_LABEL: self.label}) + "|" + json.dumps(self.layers)
            return subprocess.CompletedProcess(command, 0, out, "")
        if command[:2] == ["docker", "push"]:
            if self.on_push:
                self.on_push()
            return subprocess.CompletedProcess(command, self.push_rc, "pushed", "" if self.push_rc == 0 else "denied")
        return subprocess.CompletedProcess(command, 0, "", "")


class Push(unittest.TestCase):
    def test_pushes_when_absent_and_verifies(self):
        manifest, config = make_image()
        t = FakeRegistry()
        runner = Runner(on_push=lambda: (setattr(t, "manifest", manifest), setattr(t, "config", config)))
        state = pr.push_image(registry(t), "local:img", "2026.11.17", REV, runner)
        self.assertEqual(state.digest, digest(manifest))
        self.assertIn(["docker", "push", "reg.test/shuttlepub/emumet:2026.11.17"], runner.commands)
        self.assertFalse(any(c[:2] == ["docker", "build"] for c in runner.commands))

    def test_never_pushes_when_tag_exists(self):
        manifest, config = make_image()
        runner = Runner()
        with self.assertRaisesRegex(pr.Stop, "appeared"):
            pr.push_image(registry(FakeRegistry(manifest, config)), "local:img", "v", REV, runner)
        self.assertFalse(any(c[:2] == ["docker", "push"] for c in runner.commands))

    def test_never_pushes_when_undeterminable(self):
        runner = Runner()
        with self.assertRaises(pr.Stop):
            pr.push_image(registry(FakeRegistry(token_status=403)), "local:img", "v", REV, runner)
        self.assertFalse(any(c[:2] == ["docker", "push"] for c in runner.commands))

    def test_local_image_with_wrong_revision_is_not_pushed(self):
        runner = Runner(label=OTHER)
        with self.assertRaisesRegex(pr.Stop, "local image carries revision"):
            pr.push_image(registry(FakeRegistry()), "local:img", "v", REV, runner)
        self.assertFalse(any(c[:2] == ["docker", "push"] for c in runner.commands))

    def test_failed_push_stops_and_reports_state(self):
        t = FakeRegistry()
        reg = registry(t)
        with self.assertRaisesRegex(pr.Stop, "do NOT re-push"):
            pr.push_image(reg, "local:img", "v", REV, Runner(push_rc=1))
        self.assertTrue(any("after the failed push the registry says: absent" in line for line in reg.logs))

    def test_registry_layers_must_equal_local_layers(self):
        manifest, config = make_image(layers=LAYERS[:1])
        t = FakeRegistry()
        runner = Runner(on_push=lambda: (setattr(t, "manifest", manifest), setattr(t, "config", config)))
        with self.assertRaisesRegex(pr.Stop, "same layers"):
            pr.push_image(registry(t), "local:img", "v", REV, runner)

    def test_push_that_leaves_nothing_stops(self):
        with self.assertRaisesRegex(pr.Stop, "does not show"):
            pr.push_image(registry(FakeRegistry()), "local:img", "v", REV, Runner())


class Release(unittest.TestCase):
    def classification(self):
        return pr.Classification("same", "sha256:" + "a" * 64, "sha256:" + "b" * 64, LAYERS)

    def runner(self, releases):
        calls = []

        def run(command):
            calls.append(list(command))
            if command[:2] == ["gh", "api"]:
                return subprocess.CompletedProcess(command, 0, json.dumps([releases]), "")
            return subprocess.CompletedProcess(command, 0, "", "")

        run.calls = calls
        return run

    def test_creates_release_with_registry_digest(self):
        run = self.runner([])
        outcome = pr.create_or_verify_release(
            registry(FakeRegistry()), "ShuttlePub/Emumet", "2026.11.17", "v2026.11.17", REV, self.classification(), run, lambda m: None
        )
        self.assertEqual(outcome, "created")
        create = [c for c in run.calls if c[:3] == ["gh", "release", "create"]][0]
        self.assertIn("--verify-tag", create)

    def test_matching_release_is_noop(self):
        c = self.classification()
        body = f"digest {c.digest} commit {REV}"
        run = self.runner([{"tag_name": "v2026.11.17", "draft": False, "body": body}])
        outcome = pr.create_or_verify_release(
            registry(FakeRegistry()), "ShuttlePub/Emumet", "2026.11.17", "v2026.11.17", REV, c, run, lambda m: None
        )
        self.assertEqual(outcome, "exists")
        self.assertFalse(any(c_[:3] == ["gh", "release", "create"] for c_ in run.calls))

    def test_mismatching_or_draft_release_stops(self):
        c = self.classification()
        for release in (
            {"tag_name": "v2026.11.17", "draft": False, "body": f"commit {REV}"},
            {"tag_name": "v2026.11.17", "draft": False, "body": f"digest {c.digest}"},
            {"tag_name": "v2026.11.17", "draft": True, "body": f"digest {c.digest} commit {REV}"},
        ):
            with self.subTest(release=release), self.assertRaisesRegex(pr.Stop, "already exists"):
                pr.create_or_verify_release(
                    registry(FakeRegistry()), "ShuttlePub/Emumet", "2026.11.17", "v2026.11.17", REV, c,
                    self.runner([release]), lambda m: None,
                )

    def test_listing_failure_stops(self):
        def run(command):
            return subprocess.CompletedProcess(command, 1, "", "api down")

        with self.assertRaises(pr.Stop):
            pr.create_or_verify_release(
                registry(FakeRegistry()), "ShuttlePub/Emumet", "v", "vv", REV, self.classification(), run, lambda m: None
            )


class Cli(unittest.TestCase):
    def test_rejects_short_revision(self):
        self.assertEqual(pr.main(["classify", "--version", "v", "--revision", "abc"]), 2)


class TokenScope(unittest.TestCase):
    def scope_of(self, push_scope):
        t = FakeRegistry()
        reg = pr.Registry("reg.test", "shuttlepub/emumet", username="u", password="p", push_scope=push_scope,
                          transport=t, log=lambda m: None)
        reg.classify("v", REV)
        token_urls = [url for _, url, _ in t.requests if "/token" in url]
        self.assertEqual(len(token_urls), 1)
        return token_urls[0]

    def test_read_only_commands_ask_for_pull_only(self):
        self.assertIn("%3Apull&", self.scope_of(False) + "&")
        self.assertNotIn("push", self.scope_of(False))

    def test_push_asks_for_pull_and_push(self):
        self.assertIn("pull%2Cpush", self.scope_of(True))


if __name__ == "__main__":
    unittest.main()
