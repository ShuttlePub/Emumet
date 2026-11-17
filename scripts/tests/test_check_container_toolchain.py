import pathlib
import unittest

from loader import load

cct = load("check-container-toolchain.py", "check_container_toolchain")

D = "a" * 64
GOOD = f"""\
FROM docker.io/rust:1.97.0-bookworm@sha256:{D} AS builder
RUN test "$(rustc --version | cut -d" " -f2)" = "$(sed -n 's/^channel *= *"\\(.*\\)"/\\1/p' rust-toolchain.toml)"
FROM docker.io/debian:bookworm-slim@sha256:{D}
COPY --from=builder /x /y
"""
TOOLCHAIN = '[toolchain]\nchannel = "1.97.0"\n'


class Check(unittest.TestCase):
    def test_good(self):
        self.assertEqual(cct.check(GOOD, TOOLCHAIN), [])

    def test_version_mismatch(self):
        problems = cct.check(GOOD, '[toolchain]\nchannel = "1.98.0"\n')
        self.assertTrue(any("differs" in p for p in problems), problems)

    def test_unpinned_runtime(self):
        text = GOOD.replace(f"debian:bookworm-slim@sha256:{D}", "debian:bookworm-slim")
        self.assertTrue(any("not pinned" in p for p in cct.check(text, TOOLCHAIN)))

    def test_unpinned_builder(self):
        text = GOOD.replace(f"@sha256:{D} AS", " AS", 1)
        self.assertTrue(any("builder image" in p for p in cct.check(text, TOOLCHAIN)))

    def test_floating_channel(self):
        self.assertTrue(cct.check(GOOD, '[toolchain]\nchannel = "stable"\n'))

    def test_missing_runtime_check(self):
        text = "\n".join(l for l in GOOD.splitlines() if not l.startswith("RUN"))
        self.assertTrue(any("does not verify" in p for p in cct.check(text, TOOLCHAIN)))

    def test_short_digest(self):
        text = GOOD.replace(f"debian:bookworm-slim@sha256:{D}", "debian:bookworm-slim@sha256:abc")
        self.assertTrue(any("not pinned" in p for p in cct.check(text, TOOLCHAIN)))

    def test_repository_files_are_consistent(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        self.assertEqual(
            cct.check((root / "Dockerfile").read_text(), (root / "rust-toolchain.toml").read_text()), []
        )


if __name__ == "__main__":
    unittest.main()
