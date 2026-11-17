import hashlib
import io
import json
import subprocess
import tarfile
import unittest

from loader import load

pr = load("publish-release.py", "publish_release")

REV = "c" * 40
OTHER = "d" * 40
LAYERS = ["sha256:" + "1" * 64, "sha256:" + "2" * 64]


def digest(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def make_image(revision=REV, arch="amd64", layers=LAYERS, media=pr.MANIFEST_MEDIA_TYPES[0], version=None, entrypoint=None):
    labels = {pr.REVISION_LABEL: revision}
    if version is not None:
        labels[pr.VERSION_LABEL] = version
    config = json.dumps({
        "os": "linux", "architecture": arch,
        "config": {"Labels": labels, "Entrypoint": entrypoint or ["/app/server"], "User": "10001:10001"},
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


def local_archive(path, config, layout="docker", extra_entries=0):
    """A `docker save` archive: docker layout (<hex>.json) or OCI layout (blobs/sha256/<hex>)."""
    hexdigest = hashlib.sha256(config).hexdigest()
    name = f"{hexdigest}.json" if layout == "docker" else f"blobs/sha256/{hexdigest}"
    manifest = [{"Config": name, "RepoTags": ["x:y"], "Layers": []}] * (1 + extra_entries)
    with tarfile.open(path, "w") as archive:
        for member_name, data in (("manifest.json", json.dumps(manifest).encode()), (name, config)):
            info = tarfile.TarInfo(member_name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))


DEFAULT_CONFIG = make_image()[1]


class Runner:
    def __init__(self, label=REV, layers=LAYERS, push_rc=0, on_push=None, config=DEFAULT_CONFIG, version=None,
                 layout="docker", save_rc=0, extra_entries=0):
        self.label, self.layers, self.push_rc, self.on_push = label, layers, push_rc, on_push
        self.config, self.version, self.layout, self.save_rc = config, version, layout, save_rc
        self.extra_entries = extra_entries
        self.commands = []

    def __call__(self, command):
        self.commands.append(list(command))
        if command[:3] == ["docker", "image", "inspect"]:
            labels = {pr.REVISION_LABEL: self.label}
            if self.version is not None:
                labels[pr.VERSION_LABEL] = self.version
            out = json.dumps(labels) + "|" + json.dumps(self.layers)
            return subprocess.CompletedProcess(command, 0, out, "")
        if command[:3] == ["docker", "save", "-o"]:
            if self.save_rc == 0:
                local_archive(command[3], self.config, self.layout, self.extra_entries)
            return subprocess.CompletedProcess(command, self.save_rc, "", "" if self.save_rc == 0 else "no such image")
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
        self.assertIn(["docker", "push", "--platform", "linux/amd64", "reg.test/shuttlepub/emumet:2026.11.17"], runner.commands)
        self.assertFalse(any(c[:2] == ["docker", "build"] for c in runner.commands))

    def test_oci_layout_archive_gives_the_same_config_digest(self):
        manifest, config = make_image()
        t = FakeRegistry()
        runner = Runner(layout="oci", on_push=lambda: (setattr(t, "manifest", manifest), setattr(t, "config", config)))
        self.assertEqual(pr.push_image(registry(t), "local:img", "v", REV, runner).digest, digest(manifest))

    def test_same_layers_but_other_config_is_refused(self):
        # What the reviewer showed: identical layers and revision label, but another Entrypoint/User.
        manifest, config = make_image(entrypoint=["/bin/sh"])
        self.assertNotEqual(config, DEFAULT_CONFIG)
        t = FakeRegistry()
        runner = Runner(on_push=lambda: (setattr(t, "manifest", manifest), setattr(t, "config", config)))
        with self.assertRaisesRegex(pr.Stop, "config .* is not the config"):
            pr.push_image(registry(t), "local:img", "v", REV, runner)

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

    def test_unreadable_local_config_is_not_pushed(self):
        for runner in (Runner(save_rc=1), Runner(extra_entries=1)):
            with self.subTest(save_rc=runner.save_rc), self.assertRaises(pr.Stop):
                pr.push_image(registry(FakeRegistry()), "local:img", "v", REV, runner)
            self.assertFalse(any(c[:2] == ["docker", "push"] for c in runner.commands))

    def test_a_release_found_before_the_push_stops_it(self):
        runner = Runner()

        def precheck():
            raise pr.Stop("a Release for v already exists")

        with self.assertRaisesRegex(pr.Stop, "already exists"):
            pr.push_image(registry(FakeRegistry()), "local:img", "v", REV, runner, precheck=precheck)
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
        runner = Runner(config=config, on_push=lambda: (setattr(t, "manifest", manifest), setattr(t, "config", config)))
        with self.assertRaisesRegex(pr.Stop, "same layers"):
            pr.push_image(registry(t), "local:img", "v", REV, runner)

    def test_push_that_leaves_nothing_stops(self):
        with self.assertRaisesRegex(pr.Stop, "does not show"):
            pr.push_image(registry(FakeRegistry()), "local:img", "v", REV, Runner())


class VerifyLocal(unittest.TestCase):
    VERSION = "2026.11.17"

    def published(self, **kwargs):
        manifest, config = make_image(version=self.VERSION, **kwargs)
        return FakeRegistry(manifest, config), digest(manifest), config

    def test_a_pulled_image_identical_to_the_registry_passes(self):
        t, d, config = self.published()
        runner = Runner(config=config, version=self.VERSION)
        state = pr.verify_local_image(registry(t), "ghcr.io/x@" + d, self.VERSION, REV, d, runner)
        self.assertEqual(state.digest, d)

    def test_other_config_with_the_same_layers_is_refused(self):
        t, d, config = self.published()
        _, other = make_image(version=self.VERSION, entrypoint=["/bin/sh"])
        with self.assertRaisesRegex(pr.Stop, "config .* is not the config"):
            pr.verify_local_image(registry(t), "x", self.VERSION, REV, d, Runner(config=other, version=self.VERSION))

    def test_digest_other_than_the_verified_one_is_refused(self):
        t, d, config = self.published()
        with self.assertRaisesRegex(pr.Stop, "expected"):
            pr.verify_local_image(registry(t), "x", self.VERSION, REV, "sha256:" + "9" * 64, Runner(config=config, version=self.VERSION))

    def test_wrong_version_or_revision_labels_are_refused(self):
        t, d, config = self.published()
        with self.assertRaisesRegex(pr.Stop, "version"):
            pr.verify_local_image(registry(t), "x", self.VERSION, REV, d, Runner(config=config, version="2026.01.01"))
        with self.assertRaisesRegex(pr.Stop, "revision"):
            pr.verify_local_image(registry(t), "x", self.VERSION, REV, d, Runner(config=config, label=OTHER, version=self.VERSION))

    def test_layers_that_differ_are_refused(self):
        t, d, config = self.published()
        with self.assertRaisesRegex(pr.Stop, "same layers"):
            pr.verify_local_image(registry(t), "x", self.VERSION, REV, d, Runner(config=config, layers=LAYERS[:1], version=self.VERSION))

    def test_an_absent_tag_is_refused(self):
        with self.assertRaises(pr.Stop):
            pr.verify_local_image(registry(FakeRegistry()), "x", self.VERSION, REV, "sha256:" + "9" * 64, Runner())


TAG = "v2026.11.17"
IMAGE_REF = "reg.test/shuttlepub/emumet:2026.11.17"
DIGEST = "sha256:" + "a" * 64


def body_for(digest_=DIGEST, commit=REV, image=IMAGE_REF, extra=""):
    return (f"## Container image\n\n- Digest: `{digest_}`\n\n"
            f"{pr.render_metadata(image, digest_, commit)}\n{extra}")


class Metadata(unittest.TestCase):
    def test_round_trip_and_crlf(self):
        body = body_for()
        expected = {"image": IMAGE_REF, "digest": DIGEST, "commit": REV}
        self.assertEqual(pr.parse_metadata(body), expected)
        self.assertEqual(pr.parse_metadata(body.replace("\n", "\r\n")), expected)

    def test_unusable_blocks_stop(self):
        block = pr.render_metadata(IMAGE_REF, DIGEST, REV)
        for name, body in {
            "none": "no block here",
            "duplicate": block + "\n" + block,
            "unknown key": block.replace("commit=", "extra=1\ncommit="),
            "repeated key": block.replace("commit=", f"digest={DIGEST}\ncommit="),
            "missing key": block.replace(f"\ncommit={REV}", ""),
            "empty value": block.replace(f"commit={REV}", "commit="),
            "spaces around the value": block.replace(f"commit={REV}", f"commit= {REV}"),
            "marker mentioned elsewhere": block + "\nsee emumet-release-metadata for details",
            "not closed": block.replace("\n-->", ""),
        }.items():
            with self.subTest(name), self.assertRaises(pr.Stop):
                pr.parse_metadata(body)


class Release(unittest.TestCase):
    def classification(self):
        return pr.Classification("same", DIGEST, "sha256:" + "b" * 64, LAYERS)

    def runner(self, releases, create_rc=0, keep_listing_empty=False):
        calls = []
        listing = list(releases)

        def run(command):
            calls.append(list(command))
            if command[:2] == ["gh", "api"]:
                return subprocess.CompletedProcess(command, 0, json.dumps([listing]), "")
            if command[:3] == ["gh", "release", "create"]:
                if create_rc == 0 and not keep_listing_empty:
                    with open(command[command.index("--notes-file") + 1], encoding="utf-8") as handle:
                        listing.append({"tag_name": command[3], "draft": False, "body": handle.read()})
                return subprocess.CompletedProcess(command, create_rc, "", "" if create_rc == 0 else "boom")
            return subprocess.CompletedProcess(command, 0, "", "")

        run.calls = calls
        return run

    def call(self, run, c=None):
        return pr.create_or_verify_release(
            registry(FakeRegistry()), "ShuttlePub/Emumet", "2026.11.17", TAG, REV, c or self.classification(), run, lambda m: None
        )

    def test_creates_release_with_registry_digest_and_reads_it_back(self):
        run = self.runner([])
        self.assertEqual(self.call(run), "created")
        create = [c for c in run.calls if c[:3] == ["gh", "release", "create"]][0]
        self.assertIn("--verify-tag", create)
        self.assertNotIn("--generate-notes", create)

    def test_release_that_cannot_be_read_back_stops(self):
        with self.assertRaisesRegex(pr.Stop, "read back"):
            self.call(self.runner([], keep_listing_empty=True))

    def test_failed_create_stops(self):
        with self.assertRaisesRegex(pr.Stop, "create failed"):
            self.call(self.runner([], create_rc=1))

    def test_matching_release_is_noop(self):
        run = self.runner([{"tag_name": TAG, "draft": False, "body": body_for()}])
        self.assertEqual(self.call(run), "exists")
        self.assertFalse(any(c_[:3] == ["gh", "release", "create"] for c_ in run.calls))

    def test_expected_values_written_elsewhere_in_the_body_do_not_match(self):
        # The old check was a substring test: digest and commit mentioned anywhere were enough.
        other = body_for(digest_="sha256:" + "e" * 64, commit=OTHER, extra=f"\ndigest {DIGEST} commit {REV}\n")
        for body in (other, f"digest {DIGEST} commit {REV}", f"commit {REV}", f"digest {DIGEST}"):
            with self.subTest(body=body[:40]), self.assertRaisesRegex(pr.Stop, "already exists"):
                self.call(self.runner([{"tag_name": TAG, "draft": False, "body": body}]))

    def test_each_metadata_value_must_match(self):
        for body in (body_for(digest_="sha256:" + "e" * 64), body_for(commit=OTHER), body_for(image="reg.test/shuttlepub/emumet:other")):
            with self.subTest(body=body[-120:]), self.assertRaisesRegex(pr.Stop, "already exists"):
                self.call(self.runner([{"tag_name": TAG, "draft": False, "body": body}]))

    def test_draft_release_stops(self):
        with self.assertRaisesRegex(pr.Stop, "draft"):
            self.call(self.runner([{"tag_name": TAG, "draft": True, "body": body_for()}]))

    def test_two_releases_for_the_tag_stop(self):
        release = {"tag_name": TAG, "draft": False, "body": body_for()}
        with self.assertRaisesRegex(pr.Stop, "2 Releases"):
            self.call(self.runner([release, release]))

    def test_listing_failure_stops(self):
        def run(command):
            return subprocess.CompletedProcess(command, 1, "", "api down")

        with self.assertRaises(pr.Stop):
            self.call(run)


class RequireNoRelease(unittest.TestCase):
    def runner(self, releases, rc=0):
        def run(command):
            return subprocess.CompletedProcess(command, rc, json.dumps([releases]), "boom")

        return run

    def test_no_release_passes(self):
        pr.require_no_release(self.runner([{"tag_name": "v0.0.1"}]), "ShuttlePub/Emumet", TAG)

    def test_published_or_draft_release_stops_before_a_push(self):
        for release in ({"tag_name": TAG, "draft": False}, {"tag_name": TAG, "draft": True}):
            with self.subTest(release=release), self.assertRaisesRegex(pr.Stop, "refusing to publish an image"):
                pr.require_no_release(self.runner([release]), "ShuttlePub/Emumet", TAG)

    def test_a_failed_lookup_stops(self):
        with self.assertRaisesRegex(pr.Stop, "cannot list"):
            pr.require_no_release(self.runner([], rc=1), "ShuttlePub/Emumet", TAG)


class Cli(unittest.TestCase):
    def test_rejects_short_revision(self):
        self.assertEqual(pr.main(["classify", "--version", "v", "--revision", "abc"]), 2)

    def test_release_needs_the_verified_digest(self):
        with self.assertRaises(SystemExit):
            pr.main(["release", "--version", "v", "--revision", REV, "--tag", TAG, "--github-repository", "o/r"])

    def test_gate_needs_the_tag(self):
        with self.assertRaises(SystemExit):
            pr.main(["gate", "--version", "v", "--revision", REV, "--github-repository", "o/r"])


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
