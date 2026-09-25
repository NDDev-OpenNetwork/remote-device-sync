#!/usr/bin/env python3
"""Negative release-contract tests: all repositories/artifacts are synthetic."""
import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("rds_release", Path(__file__).with_name("release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
VERSION = "0.1.0"


class ReleaseContract(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="rds-release-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.context = contextlib.chdir(self.repo)
        self.context.__enter__()
        self.addCleanup(self.context.__exit__, None, None, None)
        for name in release.SOURCE_PATHS:
            path = Path(name)
            if path.suffix or name in ("LICENSE", "VERSION"):
                path.write_text("fixture\n")
            else:
                path.mkdir()
                (path / "fixture").write_text("fixture\n")
        Path("VERSION").write_text(VERSION + "\n")
        Path("Cargo.toml").write_text('[workspace]\nmembers=["crates/example"]\n[workspace.package]\nversion="0.1.0"\n')
        Path("crates/example").mkdir()
        Path("crates/example/Cargo.toml").write_text('[package]\nname="example"\nversion="0.1.0"\n')
        Path("Cargo.lock").write_text('version=4\n[[package]]\nname="example"\nversion="0.1.0"\n')
        Path("CHANGELOG.md").write_text('## [0.1.0]\nEngineering preview.\n')
        Path("rust-toolchain.toml").write_text('[toolchain]\nchannel="1.98.1"\n')
        self.git("init", "-q")
        self.git("config", "user.name", "Release Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.commit()
        self.git("-c", "tag.gpgSign=false", "tag", "-a", VERSION, "-m", "fixture")
        self.environment = patch.dict(os.environ, {"GITHUB_REF": "refs/tags/" + VERSION})
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.commit_oid, self.channel = release.contract(VERSION, tagged=True)

    def git(self, *args):
        return subprocess.check_output(["git", *args], stderr=subprocess.PIPE, text=True).strip()

    def commit(self):
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "-qm", "fixture")

    def bundle(self, path, target, override=None, extra=False):
        content = b"synthetic executable, not invoked" * 20
        entries = {n: (content, True) for n in release.BINARIES}
        info = dict(schema_version=1, version=VERSION, source_commit=self.commit_oid,
                    target=target, rust_toolchain=self.channel, readiness="engineering-preview",
                    features=release.FEATURES.split(","), cargo_lock_sha256=release.digest(Path("Cargo.lock")),
                    binaries={n: hashlib.sha256(content).hexdigest() for n in release.BINARIES})
        info.update(override or {})
        entries["build-info.json"] = (json.dumps(info).encode(), False)
        entries["LICENSE"] = (Path("LICENSE"), False)
        entries["Cargo.lock"] = (Path("Cargo.lock"), False)
        if extra:
            entries["unexpected"] = (b"extra", False)
        release.archive(path, f"rds-{VERSION}-{target}", entries, 0)

    def inputs(self):
        incoming = self.root / "incoming"
        incoming.mkdir()
        for target in release.TARGETS:
            self.bundle(incoming / f"remote-device-sync-{VERSION}-{target}.tar.gz", target)
        release.source_archive(VERSION, incoming / f"remote-device-sync-{VERSION}-source.tar.gz")
        (incoming / "source-sbom.spdx.json").write_text(json.dumps({"spdxVersion": "SPDX-2.3", "packages": [{"name": "example"}]}))
        return incoming

    def test_exact_version_and_lock_are_required(self):
        for name, content, error in (
            ("VERSION", "0.1.0", "VERSION mismatch"),
            ("Cargo.lock", '[[package]]\nname="example"\nversion="9.9.9"\n', "lock version mismatch"),
            ("rust-toolchain.toml", '[toolchain]\nchannel="stable"\n', "must be pinned"),
        ):
            old = Path(name).read_bytes()
            Path(name).write_text(content)
            with self.assertRaisesRegex(ValueError, error):
                release.contract(VERSION)
            Path(name).write_bytes(old)

    def test_dirty_source_and_wrong_tag_ref_are_refused(self):
        Path("LICENSE").write_text("changed")
        with self.assertRaisesRegex(ValueError, "dirty tracked"):
            release.contract(VERSION)
        self.git("checkout", "--", "LICENSE")
        with patch.dict(os.environ, {"GITHUB_REF": "refs/heads/main"}):
            with self.assertRaisesRegex(ValueError, "tag ref"):
                release.contract(VERSION, tagged=True)

    def test_lightweight_or_moved_tag_is_refused(self):
        self.git("tag", "-d", VERSION)
        self.git("-c", "tag.gpgSign=false", "tag", VERSION)
        with self.assertRaisesRegex(ValueError, "annotated"):
            release.contract(VERSION, tagged=True)
        self.git("tag", "-d", VERSION)
        self.git("-c", "tag.gpgSign=false", "tag", "-a", VERSION, "-m", "fixture")
        Path("README.md").write_text("successor")
        self.commit()
        with self.assertRaisesRegex(ValueError, "tag/source mismatch"):
            release.contract(VERSION, tagged=True)

    def test_source_includes_ops_and_shared_tests_but_no_untracked_files(self):
        Path("ops/untracked-secret").write_text("not a real secret")
        out = self.root / "source"
        release.source_bundle(VERSION, out, tagged=True)
        with tarfile.open(next(out.glob("*.tar.gz"))) as archive:
            names = archive.getnames()
        self.assertIn(f"remote-device-sync-{VERSION}/ops/fixture", names)
        self.assertIn(f"remote-device-sync-{VERSION}/tests/fixture", names)
        self.assertIn(f"remote-device-sync-{VERSION}/examples/fixture", names)
        self.assertFalse(any("untracked-secret" in n for n in names))

    def test_source_symlink_refused(self):
        Path("ops/link").symlink_to("fixture")
        self.commit()
        with self.assertRaisesRegex(ValueError, "link or gitlink"):
            release.source_archive(VERSION, self.root / "source.tar.gz")

    def test_complete_preview_manifest_and_checksums(self):
        incoming = self.inputs()
        out = self.root / "dist"
        release.finalize(VERSION, incoming, out, tagged=True)
        self.assertEqual(len(list(out.iterdir())), 7)
        manifest = json.loads((out / "release-manifest.json").read_text())
        self.assertEqual(manifest["source_commit"], self.commit_oid)
        self.assertTrue(manifest["prerelease"])
        for line in (out / "SHA256SUMS").read_text().splitlines():
            expected, name = line.split("  ")
            self.assertEqual(release.digest(out / name), expected)

    def test_missing_and_unexpected_assets_are_refused(self):
        incoming = self.inputs()
        sbom = incoming / "source-sbom.spdx.json"
        old = sbom.read_bytes()
        sbom.unlink()
        with self.assertRaisesRegex(ValueError, "inventory mismatch"):
            release.finalize(VERSION, incoming, self.root / "dist")
        sbom.write_bytes(old)
        (incoming / "extra").write_text("extra")
        with self.assertRaisesRegex(ValueError, "inventory mismatch"):
            release.finalize(VERSION, incoming, self.root / "dist")

    def test_artifact_symlink_directory_is_refused(self):
        incoming = self.inputs()
        (incoming / "link").symlink_to(self.repo, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlink"):
            release.finalize(VERSION, incoming, self.root / "dist")

    def test_tampered_source_refused(self):
        incoming = self.inputs()
        (incoming / f"remote-device-sync-{VERSION}-source.tar.gz").write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "source archive mismatch"):
            release.finalize(VERSION, incoming, self.root / "dist")

    def test_wrong_native_identity_and_digest_refused(self):
        target = release.TARGETS[0]
        for i, override in enumerate(({"source_commit": "0" * 40}, {"target": release.TARGETS[1]},
                                      {"binaries": {n: "0" * 64 for n in release.BINARIES}})):
            path = self.root / f"bad-{i}.tar.gz"
            self.bundle(path, target, override)
            with self.assertRaisesRegex(ValueError, "mismatch"):
                release.verify_binary_bundle(path, VERSION, target, self.commit_oid, self.channel)

    def test_extra_native_member_refused(self):
        path = self.root / "extra.tar.gz"
        target = release.TARGETS[0]
        self.bundle(path, target, extra=True)
        with self.assertRaisesRegex(ValueError, "member count"):
            release.verify_binary_bundle(path, VERSION, target, self.commit_oid, self.channel)

    def test_empty_sbom_refused(self):
        incoming = self.inputs()
        (incoming / "source-sbom.spdx.json").write_text('{"spdxVersion":"SPDX-2.3","packages":[]}')
        with self.assertRaisesRegex(ValueError, "empty source SBOM"):
            release.finalize(VERSION, incoming, self.root / "dist")


class CIEvidence(unittest.TestCase):
    def setUp(self):
        self.commit = "a" * 40
        self.runs = [dict(path=".github/workflows/" + name, head_sha=self.commit,
                          event="push", head_branch="main", status="completed", conclusion="success")
                     for name in ("ci.yml", "supply-chain.yml", "codeql.yml")]
        self.analyses = [dict(ref="refs/heads/main", category="/language:" + language,
                             analysis_key=".github/workflows/codeql.yml:codeql", tool={"name": "CodeQL"},
                             commit_sha=self.commit, error="", warning="")
                        for language in ("rust", "actions")]

    def test_green_workflows_with_open_alerts_are_refused(self):
        release.verify_ci_evidence(self.commit, self.runs, self.analyses, [])
        with self.assertRaisesRegex(ValueError, "unresolved code-scanning"):
            release.verify_ci_evidence(self.commit, self.runs, self.analyses, [{"number": 1}])

    def test_exact_successful_unambiguous_main_workflows_required(self):
        for field, value in (("head_sha", "b" * 40), ("event", "pull_request"),
                             ("head_branch", "topic"), ("status", "in_progress"),
                             ("conclusion", "failure")):
            runs = copy.deepcopy(self.runs)
            runs[0][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "exact-commit workflow"):
                release.verify_ci_evidence(self.commit, runs, self.analyses, [])
        for runs in (self.runs[:-1], self.runs + [self.runs[0]]):
            with self.assertRaisesRegex(ValueError, "exact-commit workflow"):
                release.verify_ci_evidence(self.commit, runs, self.analyses, [])

    def test_missing_foreign_incomplete_and_duplicate_scans_are_refused(self):
        for field, value in (("commit_sha", "b" * 40), ("ref", "refs/pull/1/merge"),
                             ("category", "/language:other"), ("analysis_key", "foreign"),
                             ("tool", {"name": "other"}), ("error", "incomplete extraction"),
                             ("warning", "partial results")):
            analyses = copy.deepcopy(self.analyses)
            analyses[0][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "CodeQL analysis"):
                release.verify_ci_evidence(self.commit, self.runs, analyses, [])
        for analyses in (self.analyses[:-1], self.analyses + [self.analyses[0]]):
            with self.assertRaisesRegex(ValueError, "CodeQL analysis"):
                release.verify_ci_evidence(self.commit, self.runs, analyses, [])

    def test_later_main_scan_cannot_supply_an_older_releases_alert_state(self):
        other = dict(self.analyses[0], commit_sha="b" * 40)
        release.verify_ci_evidence(self.commit, self.runs, self.analyses + [other], [])
        with self.assertRaisesRegex(ValueError, "CodeQL analysis"):
            release.verify_ci_evidence(self.commit, self.runs, [other] + self.analyses, [])

    def test_evidence_command_checks_alerts_on_later_pages(self):
        with tempfile.TemporaryDirectory(prefix="rds-ci-evidence-") as directory:
            paths = [Path(directory) / name for name in ("checks", "analyses", "alerts")]
            paths[0].write_text(json.dumps([{"workflow_runs": self.runs[:1]},
                                           {"workflow_runs": self.runs[1:]}]))
            paths[1].write_text(json.dumps([self.analyses[:1], self.analyses[1:]]))
            paths[2].write_text("[[], []]")
            argv = ["release.py", "evidence", "--version", VERSION,
                    "--checks", str(paths[0]), "--analyses", str(paths[1]), "--alerts", str(paths[2])]
            with patch.object(sys, "argv", argv), \
                    patch.object(release, "contract", return_value=(self.commit, "1.98.1")), \
                    contextlib.redirect_stdout(io.StringIO()):
                release.main()
                paths[2].write_text('[[], [{"number": 101}]]')
                with self.assertRaisesRegex(ValueError, "unresolved code-scanning"):
                    release.main()


if __name__ == "__main__":
    unittest.main()
