#!/usr/bin/env python3
"""Validate a release tag and print the container image version derived from it.

    scripts/validate-release.py vYYYY.MM.DD[.N] [--repo-dir DIR] [--main-ref REF] [--github-output FILE]

Rules (all of them are enforced; anything that cannot be decided is rejected):

* The tag is ``vYYYY.MM.DD`` or ``vYYYY.MM.DD.N``: a 4-digit year, 2-digit month and day, ASCII
  digits only, and ``N`` is an integer >= 1 without a leading zero (``.0``, ``.01``, signs,
  prerelease suffixes and any other trailing text are rejected).
* The date must exist in the Gregorian calendar.
* A date after "today" is rejected. "Today" is the current date in ``REFERENCE_TIMEZONE`` (a single
  constant, UTC for now); the rejection message prints the date that was used as the basis. Past
  dates are accepted without limit so that a late re-run or a delayed release stays possible.
* The commit the tag points at must be an ancestor of (or equal to) ``origin/main`` as fetched
  into the repository. A missing tag, a missing ``origin/main``, a shallow clone or any git
  failure is a rejection, never a pass.
* The image version is the tag without the leading ``v``. It is not compared with any Cargo
  version.

Exit status: 0 valid (the version is printed on stdout), 1 rejected, 2 usage error.
"""

from __future__ import annotations

import argparse
import datetime as dt
import re
import subprocess
import sys
from typing import Callable, Optional, Sequence

# The one place that decides which calendar day "today" means for the future-date check.
# Change this (for example to ``dt.timezone(dt.timedelta(hours=9), "JST")``) to change the basis.
REFERENCE_TIMEZONE: dt.tzinfo = dt.timezone.utc

DOCKER_TAG_MAX_LENGTH = 128

_TAG_PATTERN = re.compile(
    r"\Av(?P<year>[0-9]{4})\.(?P<month>[0-9]{2})\.(?P<day>[0-9]{2})(?:\.(?P<serial>[1-9][0-9]*))?\Z",
    re.ASCII,
)

DEFAULT_MAIN_REF = "refs/remotes/origin/main"


class ReleaseRejected(Exception):
    """The tag must not be released."""


def reference_today(now: Optional[dt.datetime] = None) -> dt.date:
    """The current date in REFERENCE_TIMEZONE. ``now`` is injectable for tests (aware datetime)."""
    if now is None:
        now = dt.datetime.now(dt.timezone.utc)
    if now.tzinfo is None:
        raise ValueError("now must be timezone-aware")
    return now.astimezone(REFERENCE_TIMEZONE).date()


def parse_tag(tag: str, now: Optional[dt.datetime] = None) -> str:
    """Validate the format, the date and the future check; return the version (tag without ``v``)."""
    if not isinstance(tag, str) or _TAG_PATTERN.match(tag) is None:
        raise ReleaseRejected(
            f"tag {tag!r} is not vYYYY.MM.DD or vYYYY.MM.DD.N "
            "(N >= 1, no leading zero, ASCII digits only)"
        )
    match = _TAG_PATTERN.match(tag)
    assert match is not None
    try:
        date = dt.date(int(match["year"]), int(match["month"]), int(match["day"]))
    except ValueError:
        raise ReleaseRejected(f"tag {tag!r} does not name an existing calendar date") from None
    today = reference_today(now)
    if date > today:
        raise ReleaseRejected(
            f"tag {tag!r} is dated {date.isoformat()}, which is after today "
            f"({today.isoformat()} in {REFERENCE_TIMEZONE}); a future date is not allowed"
        )
    version = tag[1:]
    if len(version) > DOCKER_TAG_MAX_LENGTH:
        raise ReleaseRejected(f"version {version!r} is longer than {DOCKER_TAG_MAX_LENGTH} characters")
    return version


GitRunner = Callable[[Sequence[str], str], "subprocess.CompletedProcess[str]"]


def _run_git(args: Sequence[str], repo_dir: str) -> "subprocess.CompletedProcess[str]":
    return subprocess.run(
        ["git", "-C", repo_dir, *args],
        capture_output=True,
        text=True,
        check=False,
    )


def resolve_commit(ref: str, repo_dir: str, run: GitRunner = _run_git) -> str:
    result = run(["rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}"], repo_dir)
    commit = result.stdout.strip()
    if result.returncode != 0 or not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ReleaseRejected(f"cannot resolve {ref} to a commit in {repo_dir}")
    return commit


def tag_commit_on_main(
    tag: str,
    repo_dir: str = ".",
    main_ref: str = DEFAULT_MAIN_REF,
    run: GitRunner = _run_git,
) -> str:
    """Return the full SHA of the tagged commit if origin/main contains it; reject otherwise."""
    shallow = run(["rev-parse", "--is-shallow-repository"], repo_dir)
    if shallow.returncode != 0 or shallow.stdout.strip() != "false":
        raise ReleaseRejected(
            "the repository is shallow or its state is unknown; fetch the full history "
            "(fetch-depth: 0) so that the ancestry check is meaningful"
        )
    tag_sha = resolve_commit(f"refs/tags/{tag}", repo_dir, run)
    main_sha = resolve_commit(main_ref, repo_dir, run)
    ancestor = run(["merge-base", "--is-ancestor", tag_sha, main_sha], repo_dir)
    if ancestor.returncode == 0:
        return tag_sha
    if ancestor.returncode == 1:
        raise ReleaseRejected(f"commit {tag_sha} of tag {tag} is not contained in {main_ref} ({main_sha})")
    raise ReleaseRejected(f"git merge-base failed (exit {ancestor.returncode}); cannot decide ancestry")


def validate(
    tag: str,
    repo_dir: str = ".",
    main_ref: str = DEFAULT_MAIN_REF,
    now: Optional[dt.datetime] = None,
    run: GitRunner = _run_git,
) -> "tuple[str, str]":
    """Return ``(version, commit_sha)`` or raise ReleaseRejected."""
    version = parse_tag(tag, now)
    commit = tag_commit_on_main(tag, repo_dir, main_ref, run)
    return version, commit


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("tag")
    parser.add_argument("--repo-dir", default=".")
    parser.add_argument("--main-ref", default=DEFAULT_MAIN_REF)
    parser.add_argument("--github-output", help="append version= and commit= lines to this file")
    args = parser.parse_args(argv)
    try:
        version, commit = validate(args.tag, args.repo_dir, args.main_ref)
    except ReleaseRejected as error:
        print(f"release rejected: {error}", file=sys.stderr)
        return 1
    if args.github_output:
        with open(args.github_output, "a", encoding="utf-8") as handle:
            handle.write(f"version={version}\ncommit={commit}\n")
    print(version)
    return 0


if __name__ == "__main__":
    sys.exit(main())
