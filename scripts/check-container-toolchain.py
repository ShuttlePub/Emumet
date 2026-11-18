#!/usr/bin/env python3
"""Check that the Dockerfile's builder Rust equals rust-toolchain.toml and that every base image
is pinned by digest.

    scripts/check-container-toolchain.py [--dockerfile Dockerfile] [--toolchain rust-toolchain.toml]

* The builder stage must be ``FROM docker.io/rust:<channel>-bookworm@sha256:<digest> AS ...`` where
  ``<channel>`` is exactly ``[toolchain] channel`` of rust-toolchain.toml (a mismatch is rejected).
* Every ``FROM`` that names an external image must carry an ``@sha256:`` digest.
  (``FROM <earlier stage>`` is allowed.)
* The compiler actually present in the builder is verified at build time by a ``RUN`` in the
  Dockerfile that compares ``rustc --version`` with rust-toolchain.toml.

Exit status: 0 consistent, 1 inconsistent.
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from typing import List, Optional, Sequence

_FROM = re.compile(r"^\s*FROM\s+(?:--platform=\S+\s+)?(?P<image>\S+)(?:\s+AS\s+(?P<name>\S+))?\s*$", re.IGNORECASE)
_DIGEST = re.compile(r"@sha256:[0-9a-f]{64}\Z")
_BUILDER = re.compile(r"\Adocker\.io/rust:(?P<version>[0-9]+\.[0-9]+\.[0-9]+)-bookworm@sha256:[0-9a-f]{64}\Z")


def toolchain_channel(text: str) -> str:
    channel = tomllib.loads(text).get("toolchain", {}).get("channel")
    if not isinstance(channel, str) or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", channel):
        raise ValueError(f"rust-toolchain.toml channel must be an exact x.y.z version, got {channel!r}")
    return channel


def check(dockerfile_text: str, toolchain_text: str) -> List[str]:
    problems: List[str] = []
    try:
        channel = toolchain_channel(toolchain_text)
    except (ValueError, tomllib.TOMLDecodeError) as error:
        return [str(error)]

    stages: List[str] = []
    builder_seen = False
    for line_number, line in enumerate(dockerfile_text.splitlines(), start=1):
        match = _FROM.match(line)
        if match is None:
            continue
        image, name = match["image"], match["name"]
        if image in stages:
            pass
        elif image.startswith("docker.io/rust:") or image.startswith("rust:"):
            builder = _BUILDER.match(image)
            if builder is None:
                problems.append(
                    f"line {line_number}: builder image {image!r} must be "
                    "docker.io/rust:<x.y.z>-bookworm@sha256:<digest>"
                )
            elif builder["version"] != channel:
                problems.append(
                    f"line {line_number}: builder Rust {builder['version']} differs from "
                    f"rust-toolchain.toml channel {channel}"
                )
            builder_seen = True
        elif _DIGEST.search(image) is None:
            problems.append(f"line {line_number}: base image {image!r} is not pinned by @sha256 digest")
        if name:
            stages.append(name)
    if not builder_seen:
        problems.append("no Rust builder stage (FROM docker.io/rust:...) found")
    if not re.search(r"^RUN\s+test\s+.*rustc --version.*rust-toolchain\.toml", dockerfile_text, re.MULTILINE):
        problems.append("the Dockerfile does not verify the builder's rustc against rust-toolchain.toml")
    return problems


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--dockerfile", default="Dockerfile")
    parser.add_argument("--toolchain", default="rust-toolchain.toml")
    args = parser.parse_args(argv)
    with open(args.dockerfile, encoding="utf-8") as handle:
        dockerfile_text = handle.read()
    with open(args.toolchain, encoding="utf-8") as handle:
        toolchain_text = handle.read()
    problems = check(dockerfile_text, toolchain_text)
    for problem in problems:
        print(f"toolchain check failed: {problem}", file=sys.stderr)
    if not problems:
        print(f"ok: builder Rust {toolchain_channel(toolchain_text)} matches rust-toolchain.toml; base images are digest-pinned")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
