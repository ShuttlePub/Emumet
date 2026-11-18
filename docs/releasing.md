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
| Which "today" | The current date in **JST (fixed UTC+9, no daylight saving time)**; the day changes at 00:00 JST, i.e. 15:00 UTC. At 15:00 UTC the JST day has already advanced: tomorrow's tag (JST) passes, the day after is rejected as a future date. Decided by the maintainers (option B). It is the single constant `REFERENCE_TIMEZONE` in `scripts/validate-release.py`; tests inject the time. The rejection message prints the JST date it used as the basis, e.g. `(2026-11-17 in JST)`. |
| Branch | The tagged commit must be contained in `origin/main`. A shallow clone, a missing ref or any git failure is a rejection, never a pass. |
| Cargo | The image version is not compared with `version` in `Cargo.toml`. |
| Serial | Two releases on one day: the second is `vYYYY.MM.DD.1`. A tag is never moved or reused. |

The release job also checks that the tag still resolves to the commit the run was started for.

## Before tagging

1. The commit to release is on `main` and every check of the **latest** run of that commit is
   green: `gh run list --repo ShuttlePub/Emumet --commit <sha>` / the PR's checks. The tag run
   rebuilds and smoke-tests the image itself, but it does not re-run the Rust test suite.
2. Pick the tag with the rules above. **Create it locally and verify it first** (the validator
   needs a full clone). Nothing is pushed in this step:
   ```sh
   git fetch origin --tags
   git tag vYYYY.MM.DD <full commit sha>          # local only
   python3 scripts/validate-release.py vYYYY.MM.DD
   ```
   If the validation fails, `git tag -d vYYYY.MM.DD` and fix the cause; nothing has left your
   machine.
3. Check that no tag, image or Release with that version exists yet (the `gate` job stops the run
   if an image is absent but a Release exists), and that no other release is running.
4. **Only now, with every check above done and the explicit approval for this release in hand**,
   push the tag. A pushed tag starts the release at once and a rejected tag cannot be removed
   again:
   ```sh
   git push origin vYYYY.MM.DD
   ```

Do not push a tag from a branch other than `main`; it will be rejected, and the rejected tag stays
in the repository (see "Recovery").

## What the tag run does

`release.yml`: `validate` -> `gate` -> (`build` -> `publish`, or `verify`) -> `release`. Runs for one tag never overlap and a
newer run never cancels a running one (`concurrency` per tag, `cancel-in-progress: false`). A
**manual** "Cancel workflow" does stop the run: the `release` job's condition uses `!cancelled()`
(not `always()`), so no Release is created after a cancel. The tag name reaches the
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
3. State `absent` (two jobs; the permission that can see drafts and write a package is held only
   by the stage that needs it):
   * **build** (`contents: read` only; no package or release permission, no token in its
     environment): builds the image, runs `scripts/smoke-container.sh` on it, records its **config
     digest** (the sha256 of the config JSON inside `docker save`, independent of the engine's image
     store), saves the image to a tar, records the **SHA-256 of the whole tar** as a job output
     (not inside the artifact), and hands the tar to the next job as a one-day artifact.
   * **publish** (`contents: write`, `packages: write`; never builds): `contents: write` is there
     for one reason, so that the Release lookup right before the push can see **draft** Releases (a
     read-only token cannot, so a draft created after the gate would be missed). It downloads the artifact and **verifies the tar's SHA-256 against the build job's output
     before `docker load`** (the config digest alone cannot see swapped layers: with the containerd
     image store a tar with the same config but other layers loads with a "wrong diff id" warning
     and exit status 0; `download-artifact` only warns about a digest mismatch). Then it logs in,
     and `publish-release.py push` (a) requires the loaded image to have the config digest the build job recorded, (b) re-classifies the tag
     (must still be absent), (c) looks for a Release (draft or not) **one last time, immediately
     before `docker push`** and stops if there is one, (d) pushes the same image (no rebuild),
     (e) reads it back and compares **revision label, platform, layer diff IDs and the config
     digest** with the local image. It outputs the digest it verified.
4. State `same` — **verify** (`contents: read`, `packages: read`): log in, `docker pull` the image
   **by digest**, run `publish-release.py verify-local` (registry digest must equal the digest from
   the gate; the pulled image's config digest, layers, revision label and version label must equal
   the registry's), then run the smoke test on the pulled image. Nothing is built or pushed.
5. **release** (`contents: write`, `packages: read`; runs when `publish` or `verify` succeeded):
   the registry must still report exactly the digest that was verified. The Release body is the
   image information and a managed block
   `<!-- emumet-release-metadata:v1` / `image=` / `digest=` / `commit=` / `-->`, **followed by the
   change log GitHub generates for the tag** (the `generate-notes` API; the body is assembled by the
   script, not by `--generate-notes`). Only the managed block is ever compared. The generated text
   is untrusted (it comes from pull request titles): it is never read for values, and the marker
   word is neutralised in it when the body is written. An existing Release for the tag is a no-op
   only if it is not a draft and its single block matches image, digest and commit **exactly**; a
   missing, duplicated, malformed or contradicting block (including a forged marker in the
   generated part), or a digest/commit mentioned only elsewhere in the text, stops the job. A
   created Release is read back and checked the same way. If the change log cannot be generated
   the job stops before creating anything.

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
| `build`: build or smoke failed | Nothing published. | Fix on `main` through a PR, then tag the new commit with the next serial. Re-running the failed job only helps for a flaky infrastructure error (the `gate` output `absent` is still true, so "Re-run failed jobs" continues with `publish`). |
| `gate`/`publish`: stopped as **other revision / platform / index** | An image with that version exists and is not ours. | Do not overwrite. Investigate who pushed it; release the next serial. |
| `gate`/`publish`: stopped as **undeterminable** (see below) | Unknown. | Read the logged HTTP status and codes first. |
| `publish`: the archive digest does not match | Nothing was loaded or pushed. The artifact differs from what `build` saved (corruption, or tampering between the jobs). | Treat it as a security finding: read the `build` and `publish` logs first. Re-running **all jobs** builds and smoke-tests again and is acceptable only after the cause is understood; re-running `publish` alone just fails the same way. |
| `publish`: `docker push` failed or its result is unknown | Possibly a partial or complete push. | The script already re-queried and logged the state. Run `classify` as above. `same`: use **Re-run all jobs**, not "Re-run failed jobs": the `gate` job already succeeded with `absent` and a failed-jobs re-run keeps that output, so `publish` would stop at "the tag exists already" (correctly: nothing is ever pushed over an image). Re-running all jobs evaluates the `gate` again, which now says `same` and sends the run to `verify` (pull by digest, verify, smoke); the release job then continues. `absent`: "Re-run failed jobs" is enough while the one-day artifact still exists (the `gate` output `absent` is still true); otherwise use **Re-run all jobs**. Anything else: stop and use the next serial. Never re-push before the state is confirmed. |
| `gate` or `publish`: stopped because **a Release exists but the image does not** | The tag has a (draft) Release and no image (`publish` stops here if the Release appeared after the gate, before anything was pushed). | Nothing was pushed. A person decides: usually delete the stray Release by hand and re-run the failed jobs (when `publish` stopped, its `gate` output `absent` is still true; use **Re-run all jobs** if the one-day artifact has expired), or release the next serial. It is never repaired automatically. |
| `verify`: pulled-image verification or smoke failed | The published image is not what the registry metadata says, or does not run. | Do not publish anything over it. Investigate (digest, config, local image store); release the next serial from a fixed commit. |
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

1. Open the `Classify the image and require that no Release exists when it is absent` step (`gate` job) and note the logged lines
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
3. Run the "Before tagging" checks and push the first tag. Watch every job of the run (`validate`, `gate`, `build`, `publish`, `release`) and keep the logs
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
