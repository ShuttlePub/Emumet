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
  push      Re-classify right before the push (must still be absent), push the already
            smoke-tested local image WITHOUT rebuilding, then read everything back from the
            registry and compare it with the local image (revision label, platform and the layer
            diff-IDs, which do not depend on the engine's image store). Prints the registry digest.
  release   Require classify == same, take the digest from the registry and create the GitHub
            Release; an existing Release is a no-op only if tag, commit and digest all match,
            otherwise stop.

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
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from typing import Callable, Dict, List, Optional, Sequence, Tuple

REVISION_LABEL = "org.opencontainers.image.revision"
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
        if platform != "linux/amd64":
            raise Stop(f"{self.image}:{version} is {platform}, expected linux/amd64")
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


def local_image_facts(image: str, run: Runner = run_command) -> Tuple[str, List[str]]:
    """The OCI revision label and the layer diff-IDs of a local image (independent of the image store)."""
    result = run(["docker", "image", "inspect", "--format", "{{json .Config.Labels}}|{{json .RootFS.Layers}}", image])
    if result.returncode != 0:
        raise Stop(f"cannot inspect the local image {image}: {result.stderr.strip()}")
    labels_json, _, layers_json = result.stdout.strip().partition("|")
    try:
        labels = json.loads(labels_json) or {}
        layers = json.loads(layers_json)
    except ValueError:
        raise Stop("cannot parse the local image metadata") from None
    return labels.get(REVISION_LABEL, ""), list(layers)


def push_image(
    registry: Registry,
    local_image: str,
    version: str,
    revision: str,
    run: Runner = run_command,
) -> Classification:
    label, layers = local_image_facts(local_image, run)
    if label != revision:
        raise Stop(f"the local image carries revision {label!r}, expected {revision}")
    state = registry.classify(version, revision)
    if state.state != "absent":
        raise Stop(f"{registry.image}:{version} appeared before the push; refusing to push over it")
    target = f"{registry.image}:{version}"
    tagged = run(["docker", "tag", local_image, target])
    if tagged.returncode != 0:
        raise Stop(f"docker tag failed: {tagged.stderr.strip()}")
    pushed = run(["docker", "push", target])
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
    if after.diff_ids != layers:
        raise Stop(
            "the registry image does not have the same layers as the smoke-tested local image "
            f"(registry {after.diff_ids}, local {layers})"
        )
    registry.log(f"push: verified {target}@{after.digest} equals the smoke-tested local image")
    return after


def release_notes(registry: Registry, version: str, digest: str, commit: str, repo_url: str) -> str:
    return (
        f"## Container image\n\n"
        f"- Image: `{registry.image}:{version}` (linux/amd64)\n"
        f"- Digest: `{digest}`\n"
        f"- Commit: `{commit}`\n\n"
        f"```sh\ndocker pull {registry.image}@{digest}\n```\n\n"
        f"Operating instructions: {repo_url}/blob/{commit}/docs/container-runtime.md\n"
    )


def find_release(run: Runner, repo: str, tag: str) -> Optional[dict]:
    """The Release (draft or not) for the tag, or None. Any failure to find out is a Stop."""
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
    existing = find_release(run, repo, tag)
    if existing is not None:
        body = existing.get("body") or ""
        problems = []
        if existing.get("draft"):
            problems.append("the Release is a draft")
        if digest not in body:
            problems.append(f"the Release does not mention the registry digest {digest}")
        if commit not in body:
            problems.append(f"the Release does not mention commit {commit}")
        if problems:
            raise Stop(f"a Release for {tag} already exists and does not match: {'; '.join(problems)}")
        log(f"release: {tag} already exists with digest {digest} and commit {commit}; nothing to do")
        return "exists"
    notes = release_notes(registry, version, digest, commit, f"https://github.com/{repo}")
    with tempfile.NamedTemporaryFile("w", suffix=".md", delete=False) as handle:
        handle.write(notes)
        notes_path = handle.name
    try:
        created = run([
            "gh", "release", "create", tag, "--repo", repo, "--verify-tag", "--title", tag,
            "--generate-notes", "--notes-file", notes_path,
        ])
    finally:
        os.unlink(notes_path)
    if created.returncode != 0:
        raise Stop(f"gh release create failed: {created.stderr.strip()[-500:]}")
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
    for name in ("classify", "push", "release"):
        command = sub.add_parser(name)
        command.add_argument("--version", required=True, help="image tag (the CalVer without the leading v)")
        command.add_argument("--revision", required=True, help="full 40-hex commit SHA the image must carry")
        command.add_argument("--registry", default="ghcr.io")
        command.add_argument("--repository", default="shuttlepub/emumet")
        command.add_argument("--insecure-http", action="store_true", help="use http (local test registry only)")
        command.add_argument("--github-output", default=os.environ.get("GITHUB_OUTPUT"))
        if name == "push":
            command.add_argument("--local-image", required=True)
        if name == "release":
            command.add_argument("--tag", required=True)
            command.add_argument("--github-repository", default=os.environ.get("GITHUB_REPOSITORY"))
    return parser


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    if not re.fullmatch(r"[0-9a-f]{40}", args.revision):
        print("--revision must be a full 40-hex SHA", file=sys.stderr)
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
        elif args.command == "push":
            state = push_image(registry, args.local_image, args.version, args.revision)
            write_outputs(args.github_output, {"state": state.state, "digest": state.digest or ""})
        else:
            if not args.github_repository:
                print("--github-repository (or GITHUB_REPOSITORY) is required", file=sys.stderr)
                return 2
            state = registry.classify(args.version, args.revision)
            if state.state != "same":
                raise Stop("the image is not in the registry yet; the Release is created only after the image exists")
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
