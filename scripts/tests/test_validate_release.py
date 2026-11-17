import datetime as dt
import os
import pathlib
import subprocess
import tempfile
import unittest

from loader import load

vr = load("validate-release.py", "validate_release")

NOW = dt.datetime(2026, 11, 17, 14, 0, tzinfo=dt.timezone.utc)


class ParseTag(unittest.TestCase):
    def test_accepts_plain_and_serial(self):
        self.assertEqual(vr.parse_tag("v2026.11.17", NOW), "2026.11.17")
        self.assertEqual(vr.parse_tag("v2026.11.17.1", NOW), "2026.11.17.1")
        self.assertEqual(vr.parse_tag("v2026.11.17.12", NOW), "2026.11.17.12")

    def test_rejects_malformed(self):
        bad = [
            "2026.11.17", "v2026.11.17.0", "v2026.11.17.01", "v2026.11.17.-1", "v2026.11.17.+1",
            "v2026.11.17.", "v2026.11.17.1.2", "v2026.1.17", "v26.11.17", "v2026.11.7",
            "v2026.11.17-rc1", "v2026.11.17 ", " v2026.11.17", "v2026.11.17\n", "V2026.11.17",
            "v2026.11.17.1a", "v２０２６.11.17", "v2026-11-17", "", "latest",
        ]
        for tag in bad:
            with self.subTest(tag=tag), self.assertRaises(vr.ReleaseRejected):
                vr.parse_tag(tag, NOW)

    def test_leap_years(self):
        late = dt.datetime(2030, 1, 1, tzinfo=dt.timezone.utc)
        self.assertEqual(vr.parse_tag("v2024.02.29", late), "2024.02.29")
        self.assertEqual(vr.parse_tag("v2000.02.29", late), "2000.02.29")
        for tag in ("v2023.02.29", "v1900.02.29", "v2100.02.29"):
            with self.subTest(tag=tag), self.assertRaises(vr.ReleaseRejected):
                vr.parse_tag(tag, late)

    def test_nonexistent_dates(self):
        for tag in ("v2026.13.01", "v2026.00.10", "v2026.04.31", "v2026.11.00", "v2026.11.31"):
            with self.subTest(tag=tag), self.assertRaises(vr.ReleaseRejected):
                vr.parse_tag(tag, NOW)

    def test_future_date_rejected_and_message_shows_basis(self):
        with self.assertRaises(vr.ReleaseRejected) as caught:
            vr.parse_tag("v2026.11.18", NOW)
        message = str(caught.exception)
        self.assertIn("2026-11-18", message)
        self.assertIn("2026-11-17", message)
        self.assertIn("UTC", message)

    def test_utc_date_boundary(self):
        just_before = dt.datetime(2026, 11, 17, 23, 59, 59, tzinfo=dt.timezone.utc)
        just_after = dt.datetime(2026, 11, 18, 0, 0, 0, tzinfo=dt.timezone.utc)
        self.assertEqual(vr.parse_tag("v2026.11.17", just_before), "2026.11.17")
        with self.assertRaises(vr.ReleaseRejected):
            vr.parse_tag("v2026.11.18", just_before)
        self.assertEqual(vr.parse_tag("v2026.11.18", just_after), "2026.11.18")

    def test_other_offset_is_one_constant(self):
        original = vr.REFERENCE_TIMEZONE
        try:
            vr.REFERENCE_TIMEZONE = dt.timezone(dt.timedelta(hours=9), "JST")
            # 2026-11-17 20:00 UTC is already 2026-11-18 05:00 in JST.
            now = dt.datetime(2026, 11, 17, 20, 0, tzinfo=dt.timezone.utc)
            self.assertEqual(vr.parse_tag("v2026.11.18", now), "2026.11.18")
            with self.assertRaises(vr.ReleaseRejected):
                vr.parse_tag("v2026.11.19", now)
        finally:
            vr.REFERENCE_TIMEZONE = original

    def test_old_date_rerun_is_accepted(self):
        self.assertEqual(vr.parse_tag("v2026.01.01.3", NOW), "2026.01.01.3")
        self.assertEqual(vr.parse_tag("v1999.12.31", NOW), "1999.12.31")

    def test_naive_now_is_refused(self):
        with self.assertRaises(ValueError):
            vr.reference_today(dt.datetime(2026, 11, 17))


def git(repo, *args, env=None):
    full_env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@e", GIT_COMMITTER_NAME="t",
                    GIT_COMMITTER_EMAIL="t@e", GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_SYSTEM="/dev/null")
    out = subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True, check=True, env=full_env)
    return out.stdout.strip()


class Ancestry(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = pathlib.Path(self.tmp.name)
        self.origin = root / "origin"
        self.origin.mkdir()
        git(self.origin, "init", "-q", "-b", "main")
        git(self.origin, "commit", "-q", "--allow-empty", "-m", "one")
        self.first = git(self.origin, "rev-parse", "HEAD")
        git(self.origin, "commit", "-q", "--allow-empty", "-m", "two")
        self.second = git(self.origin, "rev-parse", "HEAD")
        git(self.origin, "checkout", "-q", "-b", "side")
        git(self.origin, "commit", "-q", "--allow-empty", "-m", "side")
        self.side = git(self.origin, "rev-parse", "HEAD")
        git(self.origin, "checkout", "-q", "main")
        git(self.origin, "tag", "v2026.11.01", self.first)
        git(self.origin, "tag", "v2026.11.02", self.second)
        git(self.origin, "tag", "v2026.11.03", self.side)
        git(self.origin, "tag", "-a", "v2026.11.04", "-m", "annotated", self.first)
        self.clone = root / "clone"
        subprocess.run(["git", "clone", "-q", str(self.origin), str(self.clone)], check=True,
                       env=dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_SYSTEM="/dev/null"))
        git(self.clone, "fetch", "-q", "--tags", "origin")

    def check(self, tag, **kwargs):
        return vr.validate(tag, str(self.clone), now=NOW, **kwargs)

    def test_tag_on_main_is_accepted(self):
        self.assertEqual(self.check("v2026.11.01"), ("2026.11.01", self.first))
        self.assertEqual(self.check("v2026.11.02"), ("2026.11.02", self.second))

    def test_annotated_tag_resolves_to_commit(self):
        self.assertEqual(self.check("v2026.11.04"), ("2026.11.04", self.first))

    def test_tag_not_on_main_is_rejected(self):
        git(self.clone, "fetch", "-q", "origin", "side:refs/heads/side")
        with self.assertRaisesRegex(vr.ReleaseRejected, "not contained"):
            self.check("v2026.11.03")

    def test_missing_tag_is_rejected(self):
        with self.assertRaisesRegex(vr.ReleaseRejected, "cannot resolve"):
            self.check("v2026.11.09")

    def test_unfetched_main_ref_is_rejected(self):
        with self.assertRaisesRegex(vr.ReleaseRejected, "cannot resolve"):
            self.check("v2026.11.01", main_ref="refs/remotes/origin/does-not-exist")

    def test_shallow_clone_is_rejected(self):
        shallow = pathlib.Path(self.tmp.name) / "shallow"
        subprocess.run(["git", "clone", "-q", "--depth", "1", f"file://{self.origin}", str(shallow)], check=True,
                       env=dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_SYSTEM="/dev/null"))
        with self.assertRaisesRegex(vr.ReleaseRejected, "shallow"):
            vr.validate("v2026.11.02", str(shallow), now=NOW)

    def test_not_a_repository_is_rejected(self):
        with self.assertRaises(vr.ReleaseRejected):
            vr.validate("v2026.11.02", self.tmp.name + "/nonexistent", now=NOW)

    def test_ancestry_git_error_is_rejected(self):
        def fake(args, repo_dir):
            if args[0] == "merge-base":
                return subprocess.CompletedProcess(args, 128, "", "boom")
            return vr._run_git(args, repo_dir)

        with self.assertRaisesRegex(vr.ReleaseRejected, "cannot decide"):
            vr.tag_commit_on_main("v2026.11.02", str(self.clone), run=fake)

    def test_cli_writes_outputs_and_exit_codes(self):
        out = pathlib.Path(self.tmp.name) / "out"
        code = vr.main(["v2026.11.01", "--repo-dir", str(self.clone), "--github-output", str(out)])
        self.assertEqual(code, 0)
        self.assertEqual(out.read_text(), f"version=2026.11.01\ncommit={self.first}\n")
        self.assertEqual(vr.main(["v2026.11.09", "--repo-dir", str(self.clone)]), 1)


if __name__ == "__main__":
    unittest.main()
