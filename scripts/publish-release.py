#!/usr/bin/env python3
"""Publish control for the Emumet container image and its GitHub Release.

Subcommands (all of them print what they decided and why; nothing is ever overwritten):

  classify  Ask the registry about <repository>:<version> and decide:
              absent -> the tag provably does not exist (push is allowed)
              same   -> it exists, is a single linux/amd64 image whose OCI revision label equals
                        the expected commit (push is skipped)
              stop   -> anything else: other revision/platform, an image index, a registry error,
                        denied/unauthorized, an unparseable answer, a 404 without a structured
                        "unknown" error code. Exit status 1. "Cannot tell" is never "absent".
  gate      classify, and when the tag is absent also require that NO GitHub Release (draft or
            not) exists for the tag. "Release without image" is never repaired automatically.
  push      Re-classify right before the push (must still be absent), push the already
            smoke-tested local image WITHOUT rebuilding, then read everything back from the
            registry and compare it with the local image: revision label, platform, the layer
            diff-IDs and the image CONFIG DIGEST (the sha256 of the config JSON taken from
            `docker save`, which does not depend on the engine's image store). Prints the registry
            digest.
  verify-local
            For an image that was already published: compare a locally pulled copy (by digest)
            with the registry (config digest, layers, revision and version labels) and require the
            registry digest to be the expected one. The smoke test then runs on that copy.
  release   Require classify == same AND the registry digest == the digest the image job verified
            (--expected-digest), then create the GitHub Release. An existing Release is a no-op
            only if its managed metadata block (see METADATA_MARKER) matches image, digest and
            commit exactly; unknown, duplicated or contradicting metadata stops.

The registry is reached with the OCI distribution API (Bearer token flow, credentials from
REGISTRY_USERNAME / REGISTRY_PASSWORD). Every registry answer that decides something is logged as
"HTTP <status> codes=[...]" so that a first publication that stops as "undeterminable" can be
diagnosed from the log alone (see docs/releasing.md).
"""

from __future__ import annotations

import argparse
import base64
import dataclasses
import hashlib
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from typing import Callable, Dict, List, Optional, Sequence, Tuple

REVISION_LABEL = "org.opencontainers.image.revision"
VERSION_LABEL = "org.opencontainers.image.version"
PLATFORM = "linux/amd64"
METADATA_MARKER = "emumet-release-metadata:v1"
METADATA_KEYS = ("image", "digest", "commit")
MAX_CONFIG_BYTES = 4 * 1024 * 1024
MANIFEST_MEDIA_TYPES = (
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.docker.distribution.manifest.v2+json",
)
INDEX_MEDIA_TYPES = (
    "application/vnd.oci.image.index.v1+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
)
ACCEPT = ", ".join(MANIFEST_MEDIA_TYPES + INDEX_MEDIA_TYPES)
ABSENT_ERROR_CODES = ("MANIFEST_UNKNOWN", "NAME_UNKNOWN")
MAX_BLOB_BYTES = 4 * 1024 * 1024


class Stop(Exception):
    """The release must stop; the message says why. Never retried automatically."""


@dataclasses.dataclass
class Response:
    status: int
    headers: Dict[str, str]
    body: bytes


Transport = Callable[[str, str, Dict[str, str]], Response]


class TransportError(Exception):
    """The request did not produce an HTTP answer (network, TLS, timeout)."""


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[override]
        return None


def urllib_transport(method: str, url: str, headers: Dict[str, str]) -> Response:
    request = urllib.request.Request(url, method=method, headers=headers)
    opener = urllib.request.build_opener(_NoRedirect)
    try:
        with opener.open(request, timeout=30) as handle:
            return Response(handle.status, {k.lower(): v for k, v in handle.headers.items()}, handle.read(MAX_BLOB_BYTES + 1))
    except urllib.error.HTTPError as error:
        body = error.read(MAX_BLOB_BYTES + 1)
        return Response(error.code, {k.lower(): v for k, v in error.headers.items()}, body)
    except (urllib.error.URLError, OSError, TimeoutError) as error:
        raise TransportError(f"{method} {url}: {error}") from None


def sha256_digest(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def error_codes(body: bytes) -> List[str]:
    try:
        errors = json.loads(body.decode("utf-8")).get("errors", [])
        return [str(item.get("code")) for item in errors if isinstance(item, dict)]
    except (ValueError, AttributeError, UnicodeDecodeError):
        return []


@dataclasses.dataclass
class Classification:
    state: str  # absent | same
    digest: Optional[str] = None
    config_digest: Optional[str] = None
    diff_ids: Optional[List[str]] = None


class Registry:
    def __init__(
        self,
        host: str,
        repository: str,
        scheme: str = "https",
        username: Optional[str] = None,
        password: Optional[str] = None,
        push_scope: bool = False,
        transport: Transport = urllib_transport,
        log: Callable[[str], None] = lambda message: print(message, flush=True),
    ) -> None:
        self.host = host
        self.repository = repository
        self.scheme = scheme
        self.username = username
        self.password = password
        self.push_scope = push_scope
        self.transport = transport
        self.log = log
        self._token: Optional[str] = None

    @property
    def image(self) -> str:
        return f"{self.host}/{self.repository}"

    def _url(self, kind: str, reference: str) -> str:
        return f"{self.scheme}://{self.host}/v2/{self.repository}/{kind}/{reference}"

    def _fetch_token(self, challenge: str) -> str:
        params = dict(re.findall(r'(\w+)="([^"]*)"', challenge))
        realm = params.get("realm")
        if not challenge.lower().startswith("bearer") or not realm:
            raise Stop(f"unsupported registry authentication challenge: {challenge!r}")
        actions = "pull,push" if self.push_scope else "pull"
        query = {"scope": f"repository:{self.repository}:{actions}"}
        if "service" in params:
            query["service"] = params["service"]
        headers = {"Accept": "application/json"}
        if self.username and self.password:
            headers["Authorization"] = "Basic " + base64.b64encode(f"{self.username}:{self.password}".encode()).decode()
        url = f"{realm}?{urllib.parse.urlencode(query)}"
        try:
            response = self.transport("GET", url, headers)
        except TransportError as error:
            raise Stop(f"token request failed: {error}") from None
        self.log(f"registry: token request -> HTTP {response.status} codes={error_codes(response.body)}")
        if response.status != 200:
            raise Stop(
                f"the registry did not grant a token (HTTP {response.status}, codes={error_codes(response.body)}); "
                "cannot tell whether the tag exists"
            )
        try:
            payload = json.loads(response.body.decode("utf-8"))
            token = payload.get("token") or payload.get("access_token")
        except (ValueError, AttributeError, UnicodeDecodeError):
            token = None
        if not isinstance(token, str) or not token:
            raise Stop("the registry token response has no token")
        return token

    def _get(self, kind: str, reference: str, accept: str) -> Response:
        url = self._url(kind, reference)
        headers = {"Accept": accept}
        if self._token:
            headers["Authorization"] = f"Bearer {self._token}"
        try:
            response = self.transport("GET", url, headers)
            if response.status == 401 and not self._token:
                self._token = self._fetch_token(response.headers.get("www-authenticate", ""))
                headers["Authorization"] = f"Bearer {self._token}"
                response = self.transport("GET", url, headers)
            redirects = 0
            while response.status in (301, 302, 303, 307, 308) and redirects < 3:
                location = response.headers.get("location")
                if not location:
                    break
                redirects += 1
                # The redirect target (object storage) never receives the registry credentials.
                response = self.transport("GET", urllib.parse.urljoin(url, location), {})
        except TransportError as error:
            raise Stop(f"registry request failed: {error}") from None
        return response

    def classify(self, version: str, revision: str) -> Classification:
        manifest_url = self._url("manifests", version)
        response = self._get("manifests", version, ACCEPT)
        codes = error_codes(response.body)
        self.log(f"registry: GET {manifest_url} -> HTTP {response.status} codes={codes}")
        if response.status == 404:
            if any(code in ABSENT_ERROR_CODES for code in codes):
                self.log(f"classify: absent ({', '.join(c for c in codes if c in ABSENT_ERROR_CODES)} on HTTP 404)")
                return Classification("absent")
            raise Stop("HTTP 404 without a MANIFEST_UNKNOWN/NAME_UNKNOWN error code: cannot tell whether the tag exists")
        if response.status != 200:
            raise Stop(f"unexpected registry answer HTTP {response.status} codes={codes}: cannot tell whether the tag exists")

        digest = sha256_digest(response.body)
        header_digest = response.headers.get("docker-content-digest")
        if header_digest and header_digest != digest:
            raise Stop(f"manifest digest mismatch: header {header_digest}, body {digest}")
        try:
            manifest = json.loads(response.body.decode("utf-8"))
        except (ValueError, UnicodeDecodeError):
            raise Stop("the manifest is not valid JSON") from None
        media_type = manifest.get("mediaType") or response.headers.get("content-type", "").split(";")[0]
        if media_type in INDEX_MEDIA_TYPES or "manifests" in manifest:
            raise Stop(f"{self.image}:{version} is an image index; this pipeline only publishes a single linux/amd64 manifest")
        if media_type not in MANIFEST_MEDIA_TYPES:
            raise Stop(f"unexpected manifest media type {media_type!r}")
        config = manifest.get("config", {})
        config_digest = config.get("digest")
        if not isinstance(config_digest, str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", config_digest):
            raise Stop("the manifest has no valid config digest")

        blob_url = self._url("blobs", config_digest)
        blob = self._get("blobs", config_digest, "application/octet-stream")
        self.log(f"registry: GET {blob_url} -> HTTP {blob.status}")
        if blob.status != 200 or len(blob.body) > MAX_BLOB_BYTES:
            raise Stop(f"cannot read the image config (HTTP {blob.status})")
        if sha256_digest(blob.body) != config_digest:
            raise Stop("the image config does not match its digest")
        try:
            image_config = json.loads(blob.body.decode("utf-8"))
        except (ValueError, UnicodeDecodeError):
            raise Stop("the image config is not valid JSON") from None
        platform = f"{image_config.get('os')}/{image_config.get('architecture')}"
        if platform != PLATFORM:
            raise Stop(f"{self.image}:{version} is {platform}, expected {PLATFORM}")
        labels = (image_config.get("config") or {}).get("Labels") or {}
        found = labels.get(REVISION_LABEL)
        if found != revision:
            raise Stop(
                f"{self.image}:{version} already exists with revision {found!r}, expected {revision}; "
                "refusing to overwrite or reuse it"
            )
        diff_ids = (image_config.get("rootfs") or {}).get("diff_ids")
        self.log(f"classify: same (revision {revision}, linux/amd64, manifest {digest})")
        return Classification("same", digest, config_digest, diff_ids if isinstance(diff_ids, list) else None)


Runner = Callable[[Sequence[str]], "subprocess.CompletedProcess[str]"]


def run_command(command: Sequence[str]) -> "subprocess.CompletedProcess[str]":
    return subprocess.run(list(command), capture_output=True, text=True, check=False)


def local_image_facts(image: str, run: Runner = run_command) -> Tuple[Dict[str, str], List[str]]:
    """The OCI labels and the layer diff-IDs of a local image (independent of the image store)."""
    result = run(["docker", "image", "inspect", "--format", "{{json .Config.Labels}}|{{json .RootFS.Layers}}", image])
    if result.returncode != 0:
        raise Stop(f"cannot inspect the local image {image}: {result.stderr.strip()}")
    labels_json, _, layers_json = result.stdout.strip().partition("|")
    try:
        labels = json.loads(labels_json) or {}
        layers = json.loads(layers_json)
    except ValueError:
        raise Stop("cannot parse the local image metadata") from None
    if not isinstance(labels, dict) or not isinstance(layers, list):
        raise Stop("cannot parse the local image metadata")
    return labels, list(layers)


def config_digest_from_archive(path: str) -> str:
    """sha256 of the image config JSON inside a `docker save` archive (docker and OCI layouts)."""
    try:
        with tarfile.open(path) as archive:
            manifest_member = archive.extractfile("manifest.json")
            if manifest_member is None:
                raise Stop("the saved image has no manifest.json")
            entries = json.loads(manifest_member.read(MAX_CONFIG_BYTES).decode("utf-8"))
            if not isinstance(entries, list) or len(entries) != 1:
                raise Stop("the saved image must contain exactly one image (got "
                           f"{len(entries) if isinstance(entries, list) else 'an unexpected manifest.json'})")
            config_name = entries[0].get("Config")
            if not isinstance(config_name, str) or not config_name:
                raise Stop("the saved image manifest names no config")
            config_member = archive.extractfile(config_name)
            if config_member is None:
                raise Stop(f"the saved image has no {config_name}")
            data = config_member.read(MAX_CONFIG_BYTES + 1)
    except (tarfile.TarError, KeyError, ValueError, OSError) as error:
        raise Stop(f"cannot read the saved image archive: {error}") from None
    if len(data) > MAX_CONFIG_BYTES:
        raise Stop("the saved image config is unexpectedly large")
    return sha256_digest(data)


def local_config_digest(image: str, run: Runner = run_command) -> str:
    """The config digest of a local image, via `docker save`: no image-store specific field is used.

    (With the classic store the image ID equals the config digest, with the containerd store it is
    a manifest digest, so the ID cannot be used.)
    """
    with tempfile.TemporaryDirectory() as workdir:
        archive = os.path.join(workdir, "image.tar")
        saved = run(["docker", "save", "-o", archive, image])
        if saved.returncode != 0:
            raise Stop(f"cannot save the local image {image}: {saved.stderr.strip()[-500:]}")
        return config_digest_from_archive(archive)


def compare_with_registry(
    registry_state: Classification, labels: Dict[str, str], layers: List[str], config_digest: str, what: str
) -> None:
    """Stop unless the local image (labels, layers, config digest) is the registry image."""
    if registry_state.config_digest != config_digest:
        raise Stop(
            f"the registry image config {registry_state.config_digest} is not the config {config_digest} of {what} "
            "(same layers are not enough: entrypoint, user, labels and the rest must be identical)"
        )
    if registry_state.diff_ids != layers:
        raise Stop(
            f"the registry image does not have the same layers as {what} "
            f"(registry {registry_state.diff_ids}, local {layers})"
        )


def push_image(
    registry: Registry,
    local_image: str,
    version: str,
    revision: str,
    run: Runner = run_command,
    precheck: Optional[Callable[[], None]] = None,
) -> Classification:
    labels, layers = local_image_facts(local_image, run)
    label = labels.get(REVISION_LABEL, "")
    if label != revision:
        raise Stop(f"the local image carries revision {label!r}, expected {revision}")
    local_config = local_config_digest(local_image, run)
    if precheck is not None:
        precheck()
    state = registry.classify(version, revision)
    if state.state != "absent":
        raise Stop(f"{registry.image}:{version} appeared before the push; refusing to push over it")
    target = f"{registry.image}:{version}"
    tagged = run(["docker", "tag", local_image, target])
    if tagged.returncode != 0:
        raise Stop(f"docker tag failed: {tagged.stderr.strip()}")
    # --platform: with the containerd image store (the default of recent Docker) a plain push
    # uploads an image index; this pipeline publishes exactly one linux/amd64 manifest.
    pushed = run(["docker", "push", "--platform", PLATFORM, target])
    registry.log(pushed.stdout.strip()[-2000:])
    if pushed.returncode != 0:
        registry.log(f"push: docker push failed (exit {pushed.returncode}): {pushed.stderr.strip()[-1000:]}")
        try:
            outcome = registry.classify(version, revision)
            registry.log(f"push: after the failed push the registry says: {outcome.state}")
        except Stop as error:
            registry.log(f"push: after the failed push the registry state is unknown: {error}")
        raise Stop(
            "docker push failed or its result is unknown; do NOT re-push until the registry state is confirmed "
            "(see 'Recovery' in docs/releasing.md)"
        )
    after = registry.classify(version, revision)
    if after.state != "same" or after.digest is None:
        raise Stop("after the push the registry does not show the expected image")
    compare_with_registry(after, labels, layers, local_config, "the smoke-tested local image")
    registry.log(f"push: verified {target}@{after.digest} equals the smoke-tested local image (config {local_config})")
    return after


def verify_local_image(
    registry: Registry,
    local_image: str,
    version: str,
    revision: str,
    expected_digest: str,
    run: Runner = run_command,
) -> Classification:
    """A published image, pulled by digest, is exactly what the registry reports for the tag."""
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", expected_digest):
        raise Stop(f"bad expected digest {expected_digest!r}")
    state = registry.classify(version, revision)
    if state.state != "same" or state.digest != expected_digest:
        raise Stop(f"the registry reports {state.digest} for {registry.image}:{version}, expected {expected_digest}")
    labels, layers = local_image_facts(local_image, run)
    if labels.get(REVISION_LABEL) != revision:
        raise Stop(f"the pulled image carries revision {labels.get(REVISION_LABEL)!r}, expected {revision}")
    if labels.get(VERSION_LABEL) != version:
        raise Stop(f"the pulled image carries version {labels.get(VERSION_LABEL)!r}, expected {version!r}")
    compare_with_registry(state, labels, layers, local_config_digest(local_image, run), "the pulled image")
    registry.log(f"verify-local: {local_image} is {registry.image}:{version}@{state.digest} (revision, version, layers, config)")
    return state


def render_metadata(image: str, digest: str, commit: str) -> str:
    """The managed block of a Release body: the only part that is read back and compared."""
    return f"<!-- {METADATA_MARKER}\nimage={image}\ndigest={digest}\ncommit={commit}\n-->"


_METADATA_BLOCK = re.compile(r"<!-- " + re.escape(METADATA_MARKER) + r"\n(.*?)\n-->", re.DOTALL)


def parse_metadata(body: str) -> Dict[str, str]:
    """The values of the managed metadata block. Anything unexpected is a Stop, never a guess."""
    body = body.replace("\r\n", "\n")
    mentions = body.count("emumet-release-metadata")
    blocks = _METADATA_BLOCK.findall(body)
    if mentions != 1 or len(blocks) != 1:
        raise Stop(
            f"the Release body must contain exactly one {METADATA_MARKER} block "
            f"(found {len(blocks)} well-formed, {mentions} mentions)"
        )
    values: Dict[str, str] = {}
    for line in blocks[0].split("\n"):
        key, separator, value = line.partition("=")
        if not separator or key not in METADATA_KEYS or not value or value != value.strip():
            raise Stop(f"the Release metadata block has an unexpected line {line!r}")
        if key in values:
            raise Stop(f"the Release metadata block repeats {key!r}")
        values[key] = value
    missing = [key for key in METADATA_KEYS if key not in values]
    if missing:
        raise Stop(f"the Release metadata block lacks {', '.join(missing)}")
    return values


def release_notes(registry: Registry, version: str, digest: str, commit: str, repo_url: str) -> str:
    return (
        f"## Container image\n\n"
        f"- Image: `{registry.image}:{version}` (linux/amd64)\n"
        f"- Digest: `{digest}`\n"
        f"- Commit: `{commit}`\n\n"
        f"```sh\ndocker pull {registry.image}@{digest}\n```\n\n"
        f"Operating instructions: {repo_url}/blob/{commit}/docs/container-runtime.md\n\n"
        f"{render_metadata(f'{registry.image}:{version}', digest, commit)}\n"
    )


def find_release(run: Runner, repo: str, tag: str) -> Optional[dict]:
    """The Release for the tag, or None. Any failure to find out is a Stop.

    Drafts are only listed to a token that may push (contents: write); with a read-only token a
    draft is invisible, which is why the gate job runs with write access.
    """
    result = run(["gh", "api", "--paginate", "--slurp", f"repos/{repo}/releases?per_page=100"])
    if result.returncode != 0:
        raise Stop(f"cannot list the GitHub Releases: {result.stderr.strip()[-500:]}")
    try:
        pages = json.loads(result.stdout)
        releases = [item for page in pages for item in page]
    except (ValueError, TypeError):
        raise Stop("cannot parse the GitHub Releases listing") from None
    matches = [item for item in releases if item.get("tag_name") == tag]
    if len(matches) > 1:
        raise Stop(f"{len(matches)} Releases exist for {tag}")
    return matches[0] if matches else None


def require_no_release(run: Runner, repo: str, tag: str) -> None:
    """Before anything is pushed: a Release for a tag whose image does not exist is never repaired."""
    existing = find_release(run, repo, tag)
    if existing is not None:
        raise Stop(
            f"a Release for {tag} already exists (draft={bool(existing.get('draft'))}) but its image is not in the "
            "registry; refusing to publish an image under an existing Release. A person must decide "
            "(see 'Recovery' in docs/releasing.md)"
        )


def check_release_matches(release: dict, expected: Dict[str, str], tag: str) -> None:
    problems = []
    if release.get("draft"):
        problems.append("the Release is a draft")
    try:
        found = parse_metadata(release.get("body") or "")
    except Stop as error:
        problems.append(str(error))
    else:
        for key in METADATA_KEYS:
            if found[key] != expected[key]:
                problems.append(f"metadata {key} is {found[key]!r}, expected {expected[key]!r}")
    if problems:
        raise Stop(f"a Release for {tag} already exists and does not match: {'; '.join(problems)}")


def create_or_verify_release(
    registry: Registry,
    repo: str,
    version: str,
    tag: str,
    commit: str,
    classification: Classification,
    run: Runner = run_command,
    log: Callable[[str], None] = print,
) -> str:
    assert classification.digest is not None
    digest = classification.digest
    expected = {"image": f"{registry.image}:{version}", "digest": digest, "commit": commit}
    existing = find_release(run, repo, tag)
    if existing is not None:
        check_release_matches(existing, expected, tag)
        log(f"release: {tag} already exists with digest {digest} and commit {commit}; nothing to do")
        return "exists"
    notes = release_notes(registry, version, digest, commit, f"https://github.com/{repo}")
    with tempfile.NamedTemporaryFile("w", suffix=".md", delete=False) as handle:
        handle.write(notes)
        notes_path = handle.name
    try:
        # No --generate-notes: the body is exactly what is written here, so the managed block
        # cannot be duplicated by generated text (pull request titles).
        created = run(["gh", "release", "create", tag, "--repo", repo, "--verify-tag", "--title", tag,
                       "--notes-file", notes_path])
    finally:
        os.unlink(notes_path)
    if created.returncode != 0:
        raise Stop(f"gh release create failed: {created.stderr.strip()[-500:]}")
    made = find_release(run, repo, tag)
    if made is None:
        raise Stop(f"the Release for {tag} cannot be read back after it was created")
    check_release_matches(made, expected, tag)
    log(f"release: created {tag} with digest {digest}")
    return "created"


def write_outputs(path: Optional[str], values: Dict[str, str]) -> None:
    if not path:
        return
    with open(path, "a", encoding="utf-8") as handle:
        for key, value in values.items():
            handle.write(f"{key}={value}\n")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("classify", "gate", "push", "verify-local", "release"):
        command = sub.add_parser(name)
        command.add_argument("--version", required=True, help="image tag (the CalVer without the leading v)")
        command.add_argument("--revision", required=True, help="full 40-hex commit SHA the image must carry")
        command.add_argument("--registry", default="ghcr.io")
        command.add_argument("--repository", default="shuttlepub/emumet")
        command.add_argument("--insecure-http", action="store_true", help="use http (local test registry only)")
        command.add_argument("--github-output", default=os.environ.get("GITHUB_OUTPUT"))
        if name in ("gate", "push", "release"):
            command.add_argument("--tag", required=name != "push")
            command.add_argument("--github-repository", default=os.environ.get("GITHUB_REPOSITORY"))
        if name in ("push", "verify-local"):
            command.add_argument("--local-image", required=True)
        if name in ("verify-local", "release"):
            command.add_argument("--expected-digest", required=True)
    return parser


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    if not re.fullmatch(r"[0-9a-f]{40}", args.revision):
        print("--revision must be a full 40-hex SHA", file=sys.stderr)
        return 2
    if args.command in ("gate", "release") and not args.github_repository:
        print("--github-repository (or GITHUB_REPOSITORY) is required", file=sys.stderr)
        return 2
    registry = Registry(
        args.registry,
        args.repository,
        "http" if args.insecure_http else "https",
        os.environ.get("REGISTRY_USERNAME") or None,
        os.environ.get("REGISTRY_PASSWORD") or None,
        push_scope=args.command == "push",
    )
    try:
        if args.command == "classify":
            state = registry.classify(args.version, args.revision)
            write_outputs(args.github_output, {"state": state.state, "digest": state.digest or ""})
        elif args.command == "gate":
            state = registry.classify(args.version, args.revision)
            if state.state == "absent":
                require_no_release(run_command, args.github_repository, args.tag)
                registry.log(f"gate: no image and no Release for {args.tag}; the image may be published")
            write_outputs(args.github_output, {"state": state.state, "digest": state.digest or ""})
        elif args.command == "push":
            precheck = None
            if args.tag and args.github_repository:
                precheck = lambda: require_no_release(run_command, args.github_repository, args.tag)  # noqa: E731
            state = push_image(registry, args.local_image, args.version, args.revision, precheck=precheck)
            write_outputs(args.github_output, {"state": state.state, "digest": state.digest or ""})
        elif args.command == "verify-local":
            state = verify_local_image(registry, args.local_image, args.version, args.revision, args.expected_digest)
            write_outputs(args.github_output, {"state": state.state, "digest": state.digest or ""})
        else:
            state = registry.classify(args.version, args.revision)
            if state.state != "same":
                raise Stop("the image is not in the registry yet; the Release is created only after the image exists")
            if state.digest != args.expected_digest:
                raise Stop(
                    f"the registry now reports {state.digest} for the tag but the image job verified "
                    f"{args.expected_digest}; refusing to create a Release for an unverified image"
                )
            outcome = create_or_verify_release(
                registry, args.github_repository, args.version, args.tag, args.revision, state
            )
            write_outputs(args.github_output, {"release": outcome, "digest": state.digest or ""})
    except Stop as error:
        print(f"STOP: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
