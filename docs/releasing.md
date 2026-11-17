# Releasing the container image

A pushed tag `vYYYY.MM.DD` (or `vYYYY.MM.DD.N`) publishes
`ghcr.io/shuttlepub/emumet:YYYY.MM.DD[.N]` (linux/amd64 only, no `latest`) and creates a GitHub
Release that names the image digest. Nothing is published from a pull request. Deploying the image
(Kubernetes, compose, Ory configuration) is out of scope here; see
[container-runtime.md](container-runtime.md) for how the image behaves.

Files: `Dockerfile`, `.dockerignore`, `scripts/` (`validate-release.py`, `publish-release.py`,
`check-container-toolchain.py`, `smoke-container.sh`, `test-publish-local-registry.sh`,
`tests/`), `.github/workflows/container-ci.yml` (pull requests) and
`.github/workflows/release.yml` (tags).

## Version rules (CalVer)

| Rule | Detail |
| --- | --- |
| Format | `vYYYY.MM.DD` or `vYYYY.MM.DD.N`. ASCII digits only; `N` is an integer >= 1 without a leading zero (`.0`, `.01`, signs, `-rc1` and other suffixes are rejected). |
| Date | Must exist in the Gregorian calendar (`v2026.02.29` is rejected). |
| Future | A date after "today" is rejected. Past dates are always accepted, so a late re-run or a delayed release stays possible. |
| Which "today" | The current date in **UTC**. This is the single constant `REFERENCE_TIMEZONE` in `scripts/validate-release.py`; tests inject the time. The rejection message prints the date it used as the basis. *Status: UTC is provisional until the maintainers decide between UTC, a fixed JST, or "UTC plus one day"; changing it means editing that one constant and its test.* |
| Branch | The tagged commit must be contained in `origin/main`. A shallow clone, a missing ref or any git failure is a rejection, never a pass. |
| Cargo | The image version is not compared with `version` in `Cargo.toml`. |
| Serial | Two releases on one day: the second is `vYYYY.MM.DD.1`. A tag is never moved or reused. |

The release job also checks that the tag still resolves to the commit the run was started for.

## Before tagging

1. The commit to release is on `main` and every check of the **latest** run of that commit is
   green: `gh run list --repo ShuttlePub/Emumet --commit <sha>` / the PR's checks. The tag run
   rebuilds and smoke-tests the image itself, but it does not re-run the Rust test suite.
2. Pick the tag with the rules above and verify it locally if in doubt (needs a full clone):
   `git fetch origin --tags && python3 scripts/validate-release.py vYYYY.MM.DD`.
3. Check that no tag, image or Release with that version exists yet.

Do not push a tag from a branch other than `main`; it will be rejected, and the rejected tag stays
in the repository (see "Recovery").

## What the tag run does

`release.yml`: `validate` -> `image` -> `release`. Runs for one tag never overlap and are never
cancelled (`concurrency` per tag, `cancel-in-progress: false`); the tag name reaches the scripts
through `env`, never through `${{ }}` inside a shell command.

1. **validate** (`contents: read`): script tests, `validate-release.py`.
2. **image** (`contents: read`, `packages: write`; the only job that can write a package):
   `publish-release.py classify` asks the registry about the tag.
   * `absent` (HTTP 404 *with* `MANIFEST_UNKNOWN`/`NAME_UNKNOWN`): build, run
     `scripts/smoke-container.sh` on that image, log in, re-query immediately before the push
     (it must still be absent), push the **same** smoke-tested image (no rebuild), then read it
     back from the registry and compare revision label, platform and layer diff IDs with the local
     image.
   * `same` (single linux/amd64 manifest whose `org.opencontainers.image.revision` equals the
     commit): skip build, smoke and push.
   * anything else **stops**: other revision, other platform, an image index, a non-404 error,
     denied/unauthorized, a 404 without a "manifest unknown" code, an unparseable answer, a
     network failure. "Cannot tell" is never treated as "absent".
3. **release** (`contents: write`, `packages: read`): classify must say `same`; the digest in the
   Release notes is the one the registry reports (not a local one). An existing Release for the tag
   is a no-op if it is not a draft and mentions both the digest and the commit; otherwise the job
   stops.

Provenance and SBOM attestations are switched off (`--provenance=false --sbom=false`) so the tag
points to one plain manifest that the script can compare; they can be added later as a deliberate
change.

### Where the decision is recorded

Every registry answer that decides something is printed in the job log as
`registry: GET <url> -> HTTP <status> codes=[...]` (and
`registry: token request -> HTTP <status> codes=[...]`), followed by
`classify: absent|same ...` or `STOP: <reason>`. Keep that log when you report a release; it is the
evidence for "the tag did not exist" or for why the run stopped.

## Recovery

A tag is never moved or deleted to fix a release: pick the next serial. Check the state first
(from a clone with `REGISTRY_USERNAME`/`REGISTRY_PASSWORD` set to a GitHub user and a token that
can read the package):

```sh
python3 scripts/publish-release.py classify --version YYYY.MM.DD --revision <full commit sha>
gh release view vYYYY.MM.DD --repo ShuttlePub/Emumet
```

| Where it stopped | State | What to do |
| --- | --- | --- |
| `validate` | Nothing published. | Fix the cause (wrong date, not on `main`, shallow checkout). The rejected tag stays; release the next serial (`vYYYY.MM.DD.N`) on the right commit. |
| `image`: build or smoke failed | Nothing published. | Fix on `main` through a PR, then tag the new commit with the next serial. Re-running the failed job only helps for a flaky infrastructure error. |
| `image`: stopped as **other revision / platform / index** | An image with that version exists and is not ours. | Do not overwrite. Investigate who pushed it; release the next serial. |
| `image`: stopped as **undeterminable** (see below) | Unknown. | Read the logged HTTP status and codes first. |
| `image`: `docker push` failed or its result is unknown | Possibly a partial or complete push. | The script already re-queried and logged the state. Run `classify` as above. `same`: re-run the failed jobs; the image job skips the push and the release job continues. `absent`: re-run the failed jobs. Anything else: stop and use the next serial. Never re-push before the state is confirmed. |
| `release` failed | Image is published. | Re-run the failed jobs; the Release is created from the registry digest (an identical existing Release is a no-op). A mismatching or draft Release must be reviewed by a person. |

### The first publication stopped as "undeterminable"

Before the first publication the package does not exist, and GHCR answers an unauthenticated
request for a missing package exactly like one for a private package (verified 2026-11-17:
`GET /v2/shuttlepub/emumet/manifests/<tag>` -> 401 `UNAUTHORIZED`; an anonymous token request ->
`DENIED`). With the workflow's token the job should see HTTP 404 + `NAME_UNKNOWN` or
`MANIFEST_UNKNOWN`; that authenticated answer for a not-yet-existing package has **not** been
observed yet, so the first run is the real test. If it stops:

1. Open the `Registry state of this version` step and note the logged lines
   (`token request -> HTTP ...`, `GET ... -> HTTP ... codes=[...]`).
2. Confirm in the organisation's *Packages* page that `emumet` really does not exist.
3. Nothing was pushed, so nothing needs cleaning. If the logged answer is a legitimate "absent"
   that the script does not recognise, fix `ABSENT_ERROR_CODES` / the rule in
   `scripts/publish-release.py` (with a test) through a normal PR, merge it, and release the
   **next serial**: re-running the old run would use the old script from the old tag's commit.
4. If the token was denied (HTTP 403 `DENIED`), check the organisation's Actions package
   permissions and that the workflow has `packages: write`; then release the next serial.

## Tag protection (ruleset) — to be configured by an administrator

Not configured by the pull request that added this mechanism. Settings -> Rules -> Rulesets ->
New tag ruleset: target `Tags`, pattern `v*`, enforcement `Active`, rules **Restrict updates** and
**Restrict deletions** (optionally **Restrict creations** to the maintainers who may release).
Without it, a moved tag can no longer match its published image, and the scripts refuse to
repair that.

## First publication (after the pull request is merged; explicit approval required)

These steps change shared state and are not part of the pull request. Do them only with the
maintainers' explicit approval:

1. Configure the tag ruleset above.
2. Run the "Before tagging" checks and push the first tag. Watch the three jobs and keep the log
   (see "Where the decision is recorded").
3. A new GHCR package is **private** by default. Making it public (anonymous `docker pull`) is a
   separate setting (Package settings -> Change visibility) and also needs explicit approval.
4. Record the first release (tag, digest, run URL) in the repository's usual operations log.

### Why the push uses `--platform linux/amd64`
Recent Docker versions (observed: 29.4.2 on `ubuntu-latest`, 2026-11-17, PR CI of the release PR) store
images with the containerd image store, and a plain `docker push` then uploads an **image index**. The
pipeline publishes exactly one single-platform manifest, so `publish-release.py push` runs
`docker push --platform linux/amd64`, which uploads that one manifest only (no index, no attestations).
The classifier still refuses an index it finds in the registry.
