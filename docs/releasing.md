# Releasing the container image

A pushed tag `vYYYY.MM.DD` (or `vYYYY.MM.DD.N`) publishes
`ghcr.io/shuttlepub/emumet:YYYY.MM.DD[.N]` (linux/amd64 only, no `latest`) and creates a GitHub
Release that names the image digest. Nothing is published from a pull request. Deploying the image
(Kubernetes, compose, Ory configuration) is out of scope here; see
[container-runtime.md](container-runtime.md) for how the image behaves.

Files: `Dockerfile`, `.dockerignore`, `scripts/` (`validate-release.py`, `publish-release.py`,
`check-container-toolchain.py`, `smoke-container.sh`, `test-publish-local-registry.sh`,
`tests/`), `.github/workflows/container-ci.yml` (pull requests) and
`.github/workflows/release.yml` (tags) and `.github/workflows/release-probe.yml` (manual, read-only).

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
2. Pick the tag with the rules above. **Create it locally first, verify it, and only then push
   it** (a pushed tag starts the release at once and a rejected tag cannot be removed again; the
   validator needs a full clone):
   ```sh
   git fetch origin --tags
   git tag vYYYY.MM.DD <full commit sha>          # local only
   python3 scripts/validate-release.py vYYYY.MM.DD
   git push origin vYYYY.MM.DD                    # only when the validation passed
   ```
   If the validation fails, `git tag -d vYYYY.MM.DD` and fix the cause; nothing has left your
   machine.
3. Check that no tag, image or Release with that version exists yet (the `gate` job stops the run
   if an image is absent but a Release exists).

Do not push a tag from a branch other than `main`; it will be rejected, and the rejected tag stays
in the repository (see "Recovery").

## What the tag run does

`release.yml`: `validate` -> `gate` -> `image` -> `release`. Runs for one tag never overlap and are
never cancelled (`concurrency` per tag, `cancel-in-progress: false`); the tag name reaches the
scripts through `env`, never through `${{ }}` inside a shell command.

1. **validate** (`contents: read`): script tests, `validate-release.py`.
2. **gate** (`contents: write`, `packages: read`; creates nothing): `publish-release.py gate` asks
   the registry about the tag.
   * `absent` (HTTP 404 *with* `MANIFEST_UNKNOWN`/`NAME_UNKNOWN`): **no GitHub Release (draft or
     published) may exist for the tag** and the lookup must succeed; otherwise the run stops
     *before* anything is built or pushed. "A Release without its image" is never repaired
     automatically. (`contents: write` is needed only because a read-only token cannot see draft
     Releases.)
   * `same` (single linux/amd64 manifest whose `org.opencontainers.image.revision` equals the
     commit): the digest is passed on.
   * anything else **stops**: other revision, other platform, an image index, a non-404 error,
     denied/unauthorized, a 404 without a "manifest unknown" code, an unparseable answer, a
     network failure. "Cannot tell" is never treated as "absent".
3. **image** (`contents: read`, `packages: write`; the only job that can write a package):
   * state `absent`: build, run `scripts/smoke-container.sh` on that image, log in, re-check
     right before the push that the tag is still absent and that no *published* Release exists
     (this job's token cannot see drafts; the `gate` job did), push the **same** smoke-tested image
     (no rebuild), then read it back from the registry and compare **revision label, platform,
     layer diff IDs and the image config digest** with the local image. The local config digest is
     the sha256 of the config JSON inside `docker save`; it does not depend on the engine's image
     store, and it catches an image with the same layers but a different Entrypoint, User or
     labels.
   * state `same`: log in, `docker pull` the image **by digest**, run
     `publish-release.py verify-local` (registry digest must equal the digest from the gate; the
     pulled image's config digest, layers, revision label and version label must equal the
     registry's), then run the smoke test on the pulled image. Nothing is built or pushed.
   * the job outputs the digest it verified.
4. **release** (`contents: write`, `packages: read`): the registry must still report exactly that
   digest. The Release body contains a managed block
   `<!-- emumet-release-metadata:v1` / `image=` / `digest=` / `commit=` / `-->`, written without
   `--generate-notes` so that nothing else can add a second block. An existing Release for the tag
   is a no-op only if it is not a draft and its single block matches image, digest and commit
   **exactly**; a missing, duplicated, malformed or contradicting block, or a digest/commit
   mentioned only elsewhere in the text, stops the job. A created Release is read back and checked
   the same way.

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
| `image`: `docker push` failed or its result is unknown | Possibly a partial or complete push. | The script already re-queried and logged the state. Run `classify` as above. `same`: re-run the failed jobs; the image job pulls the published image by digest, verifies and smoke-tests it instead of pushing, and the release job continues. `absent`: re-run the failed jobs. Anything else: stop and use the next serial. Never re-push before the state is confirmed. |
| `gate`: stopped because **a Release exists but the image does not** | The tag has a (draft) Release and no image. | Nothing was built or pushed. A person decides: usually delete the stray Release by hand and re-run the failed jobs, or release the next serial. It is never repaired automatically. |
| `image`: `same` but the pulled-image verification or smoke failed | The published image is not what the registry metadata says, or does not run. | Do not publish anything over it. Investigate (digest, config, local image store); release the next serial from a fixed commit. |
| `release` failed | Image is published. | Re-run the failed jobs; the Release is created from the registry digest (an identical existing Release is a no-op). A mismatching or draft Release must be reviewed by a person. |

### Rolling back after a release

Rolling the **image** back to an older version is not always possible: migrations are applied at
start-up and are not reversible, so an older image started against a database that a newer image
already migrated can refuse to start or misbehave (it does not know the newer schema). Before
rolling back, check whether the newer release applied migrations the older one lacks (compare
`migrations/` between the two commits); if it did, restore the database from a backup taken
before the upgrade, or fix forward with the next release. Taking a database backup before
upgrading an environment is the operator's responsibility.

### The first publication stopped as "undeterminable"

Before the first publication the package does not exist, and GHCR answers an unauthenticated
request for a missing package exactly like one for a private package (verified 2026-11-17:
`GET /v2/shuttlepub/emumet/manifests/<tag>` -> 401 `UNAUTHORIZED`; an anonymous token request ->
`DENIED`). With the workflow's token the job should see HTTP 404 + `NAME_UNKNOWN` or
`MANIFEST_UNKNOWN`; that authenticated answer for a not-yet-existing package has **not** been
observed yet. To see it **before** the first release, run the read-only probe workflow by hand
(after this mechanism is merged; workflow_dispatch needs the file on the default branch):
`gh workflow run release-probe.yml --repo ShuttlePub/Emumet`. It asks the registry about a tag
that is never pushed with `packages: read` only and creates nothing; the expected log line is
`classify: absent (... on HTTP 404)`. Otherwise the first run is the real test. If it stops:

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
2. Optionally run the read-only probe (`release-probe.yml`, see above).
3. Run the "Before tagging" checks and push the first tag. Watch the four jobs and keep the log
   (see "Where the decision is recorded").
4. A new GHCR package is **private** by default. Making it public (anonymous `docker pull`) is a
   separate setting (Package settings -> Change visibility) and also needs explicit approval.
5. After it is public, verify it as an anonymous user would: with an **empty Docker config** (no
   login), pull by tag and by digest, check that both resolve to the digest in the Release, and run
   the smoke test on the digest reference:
   ```sh
   export DOCKER_CONFIG="$(mktemp -d)"            # no credentials
   docker pull ghcr.io/shuttlepub/emumet:YYYY.MM.DD
   docker pull ghcr.io/shuttlepub/emumet@sha256:<digest from the Release>
   docker image inspect --format '{{index .RepoDigests 0}}' ghcr.io/shuttlepub/emumet:YYYY.MM.DD
   scripts/smoke-container.sh ghcr.io/shuttlepub/emumet@sha256:<digest from the Release>
   rm -rf "$DOCKER_CONFIG"; unset DOCKER_CONFIG
   ```
6. Record the first release (tag, digest, run URL) in the repository's usual operations log.

### Why the push uses `--platform linux/amd64`
Recent Docker versions (observed: 29.4.2 on `ubuntu-latest`, 2026-11-17, PR CI of the release PR) store
images with the containerd image store, and a plain `docker push` then uploads an **image index**. The
pipeline publishes exactly one single-platform manifest, so `publish-release.py push` runs
`docker push --platform linux/amd64`, which uploads that one manifest only (no index, no attestations).
The classifier still refuses an index it finds in the registry.
