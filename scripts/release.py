#!/usr/bin/env python3
"""Build-time release assembly only; installed RDS programs remain native Rust.

No network access or publication here. GitHub Actions supplies bounded build
artifacts; this script refuses version/source/target drift before publication.
"""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-apple-darwin")
BINARIES = ("rds", "rds-agent", "rds-relay", "rds-server")
FEATURES = "rds-agent/transport-noq,rds-cli/transport-noq,rds-relay/owned-relay,rds-server/owned-relay"
SOURCE_PATHS = ("README.md", "LICENSE", "VERSION", "CHANGELOG.md", "AGENTS.md",
                "deny.toml", "rust-toolchain.toml", "Cargo.toml", "Cargo.lock",
                "crates", "tests", "examples", "deploy", "docs", "scripts", ".github", "ops")


def run(*args):
    return subprocess.check_output(args, text=True, timeout=60).strip()


def require(ok, reason):
    if not ok:
        raise ValueError(reason)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def contract(version, tagged=False):
    require(re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version),
            "expected numeric SemVer")
    require(Path("VERSION").read_bytes() == (version + "\n").encode(), "VERSION mismatch")
    manifest = tomllib.loads(Path("Cargo.toml").read_text())
    require(manifest["workspace"]["package"]["version"] == version, "Cargo version mismatch")
    lock = tomllib.loads(Path("Cargo.lock").read_text())
    for member in manifest["workspace"]["members"]:
        name = tomllib.loads((Path(member) / "Cargo.toml").read_text())["package"]["name"]
        matches = [p for p in lock["package"] if p["name"] == name and "source" not in p]
        require(len(matches) == 1 and matches[0]["version"] == version, f"lock version mismatch: {name}")
    channel = tomllib.loads(Path("rust-toolchain.toml").read_text())["toolchain"]["channel"]
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", channel), "Rust toolchain must be pinned")
    notes(version)
    commit = run("git", "rev-parse", "HEAD")
    require(re.fullmatch(r"[0-9a-f]{40}", commit), "invalid source OID")
    require(not run("git", "status", "--porcelain", "--untracked-files=no"), "dirty tracked source")
    if tagged:
        require(os.environ.get("GITHUB_REF") == f"refs/tags/{version}", "release must run from tag ref")
        require(run("git", "cat-file", "-t", f"refs/tags/{version}") == "tag", "annotated tag required")
        require(run("git", "rev-parse", f"refs/tags/{version}^{{commit}}") == commit, "tag/source mismatch")
    return commit, channel


def notes(version):
    content = Path("CHANGELOG.md").read_text()
    headings = list(re.finditer(r"^## \[([^\]]+)\].*$", content, re.M))
    selected = [i for i, h in enumerate(headings) if h.group(1) == version]
    require(len(selected) == 1, "expected exactly one release heading")
    i = selected[0]
    end = headings[i + 1].start() if i + 1 < len(headings) else len(content)
    result = content[headings[i].end():end].strip()
    require(result and "engineering preview" in result.lower(), "preview release notes required")
    return result + "\n"


def verify_ci_evidence(commit, runs, analyses, alerts):
    """A successful scanner invocation does not mean its findings are resolved."""
    require(all(isinstance(items, list) for items in (runs, analyses, alerts)), "invalid CI evidence")
    for workflow in ("ci.yml", "supply-chain.yml", "codeql.yml"):
        matches = [r for r in runs if r["path"] == ".github/workflows/" + workflow
                   and r["head_sha"] == commit and r["event"] == "push" and r["head_branch"] == "main"]
        require(len(matches) == 1 and matches[0]["status"] == "completed"
                and matches[0]["conclusion"] == "success",
                "missing/ambiguous/unsuccessful exact-commit workflow: " + workflow)
    for language in ("rust", "actions"):
        matches = [a for a in analyses if a["ref"] == "refs/heads/main"
                   and a["category"] == "/language:" + language
                   and a["analysis_key"] == ".github/workflows/codeql.yml:codeql"
                   and a["tool"]["name"] == "CodeQL"]
        # GitHub returns newest first. Alerts on main must describe this
        # source, not a later revision that already fixed its vulnerabilities.
        require(sum(a["commit_sha"] == commit for a in matches) == 1
                and matches[0]["commit_sha"] == commit
                and matches[0]["error"] == "" and matches[0]["warning"] == "",
                "missing/ambiguous/incomplete exact-commit CodeQL analysis: " + language)
    require(not alerts, "unresolved code-scanning alerts on main")


def new_output(path):
    path.mkdir(parents=True, exist_ok=False)
    return path


def regular(path):
    require(path.is_file() and not path.is_symlink(), f"expected regular file: {path.name}")


def archive(path, prefix, entries, timestamp):
    # entries: name -> (bytes or source Path, executable). Stable order, owner,
    # modes and timestamps; reproducible packaging does not imply reproducible binaries.
    with path.open("xb") as output, gzip.GzipFile(fileobj=output, filename="", mode="wb", mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.USTAR_FORMAT) as tar:
            for name, (source, executable) in sorted(entries.items()):
                data = source.read_bytes() if isinstance(source, Path) else source
                entry = tarfile.TarInfo(f"{prefix}/{name}")
                entry.size, entry.mtime, entry.mode = len(data), timestamp, 0o755 if executable else 0o644
                tar.addfile(entry, io.BytesIO(data))


def source_archive(version, path):
    # git archive includes only the chosen tracked paths, including observability.
    listing = subprocess.check_output(["git", "ls-files", "--stage", "--", *SOURCE_PATHS], text=True)
    for line in listing.splitlines():
        require(line.split()[0] in ("100644", "100755"), "source contains a link or gitlink")
    with path.open("xb") as dest, gzip.GzipFile(fileobj=dest, filename="", mode="wb", mtime=0) as gz:
        proc = subprocess.Popen(["git", "archive", "--format=tar", f"--prefix=remote-device-sync-{version}/",
                                 "HEAD", "--", *SOURCE_PATHS], stdout=subprocess.PIPE)
        try:
            while data := proc.stdout.read(1024 * 1024):
                gz.write(data)
            require(proc.wait(timeout=60) == 0, "git archive failed")
        finally:
            proc.stdout.close()
            if proc.poll() is None:
                proc.kill()
                proc.wait()


def source_bundle(version, out, tagged=False):
    contract(version, tagged=tagged)
    new_output(out)
    path = out / f"remote-device-sync-{version}-source.tar.gz"
    source_archive(version, path)
    # Extract only our generated regular-file archive for the source SBOM scan.
    with tarfile.open(path, "r:gz") as tar:
        tar.extractall(out / "source", filter="data")


def binary_bundle(version, target, binary_dir, out, tagged=False):
    require(target in TARGETS, "unsupported target")
    require(os.environ.get("RELEASE_FEATURES", FEATURES) == FEATURES, "build feature profile mismatch")
    commit, channel = contract(version, tagged=tagged)
    require(run("rustc", "--version").split()[1] == channel, "compiler version mismatch")
    host = next(line.removeprefix("host: ") for line in run("rustc", "-vV").splitlines() if line.startswith("host: "))
    require(host == target, "native target/runner mismatch")
    entries, hashes = {}, {}
    for name in BINARIES:
        path = binary_dir / name
        regular(path)
        require(path.stat().st_mode & 0o111 and path.stat().st_size > 256, f"invalid executable: {name}")
        require(run(str(path.resolve()), "--version") == f"{name} {version}", f"binary version/name mismatch: {name}")
        hashes[name] = digest(path)
        entries[name] = (path, True)
    info = dict(schema_version=1, version=version, source_commit=commit, target=target,
                rust_toolchain=channel, features=FEATURES.split(","), readiness="engineering-preview",
                binaries=hashes, cargo_lock_sha256=digest(Path("Cargo.lock")))
    entries["build-info.json"] = ((json.dumps(info, sort_keys=True, indent=2) + "\n").encode(), False)
    for name in ("LICENSE", "Cargo.lock"):
        entries[name] = (Path(name), False)
    new_output(out)
    archive(out / f"remote-device-sync-{version}-{target}.tar.gz", f"rds-{version}-{target}", entries,
            int(run("git", "show", "-s", "--format=%ct", "HEAD")))


def verify_binary_bundle(path, version, target, commit, channel):
    regular(path)
    prefix = f"rds-{version}-{target}/"
    expected = set(BINARIES) | {"build-info.json", "LICENSE", "Cargo.lock"}
    with tarfile.open(path, "r:gz") as tar:
        members = tar.getmembers()
        require(len(members) == len(expected), "binary archive member count mismatch")
        require({m.name for m in members} == {prefix + n for n in expected}, "binary archive member mismatch")
        require(all(m.isfile() and m.size <= 512 * 1024 * 1024 for m in members), "unsafe binary archive member")
        require(tar.getmember(prefix + "build-info.json").size <= 64 * 1024, "oversized build info")
        info = json.load(tar.extractfile(prefix + "build-info.json"))
        for key, value in dict(schema_version=1, version=version, source_commit=commit, target=target,
                               rust_toolchain=channel, features=FEATURES.split(","), readiness="engineering-preview").items():
            require(info.get(key) == value, f"build info mismatch: {key}")
        require(set(info.get("binaries", {})) == set(BINARIES), "binary manifest mismatch")
        for name in BINARIES:
            require(tar.getmember(prefix + name).mode == 0o755, "executable mode mismatch")
            with tar.extractfile(prefix + name) as stream:
                require(hashlib.file_digest(stream, "sha256").hexdigest() == info["binaries"][name], "binary digest mismatch")
        with tar.extractfile(prefix + "Cargo.lock") as stream:
            require(hashlib.file_digest(stream, "sha256").hexdigest() == digest(Path("Cargo.lock")) == info["cargo_lock_sha256"], "lock digest mismatch")
        with tar.extractfile(prefix + "LICENSE") as stream:
            require(hashlib.file_digest(stream, "sha256").hexdigest() == digest(Path("LICENSE")), "license digest mismatch")


def finalize(version, incoming, out, tagged=False):
    commit, channel = contract(version, tagged=tagged)
    expected = {f"remote-device-sync-{version}-{t}.tar.gz" for t in TARGETS}
    expected |= {f"remote-device-sync-{version}-source.tar.gz", "source-sbom.spdx.json"}
    require(incoming.is_dir() and not incoming.is_symlink(), "invalid input directory")
    all_paths = list(incoming.rglob("*"))
    require(not any(p.is_symlink() for p in all_paths), "symlink in artifact input")
    paths = [p for p in all_paths if not p.is_dir()]
    require(len(paths) == len(expected) and {p.name for p in paths} == expected, "release asset inventory mismatch")
    for path in paths:
        regular(path)
    mapped = {p.name: p for p in paths}
    for target in TARGETS:
        verify_binary_bundle(mapped[f"remote-device-sync-{version}-{target}.tar.gz"], version, target, commit, channel)
    # Regenerate the source payload from this checkout, rather than trusting
    # an archive filename or a producer-supplied commit string.
    with tempfile.TemporaryDirectory(prefix="rds-source-check-") as temporary:
        expected_source = Path(temporary) / "source.tar.gz"
        source_archive(version, expected_source)
        require(digest(mapped[f"remote-device-sync-{version}-source.tar.gz"]) == digest(expected_source), "source archive mismatch")
    sbom = json.loads(mapped["source-sbom.spdx.json"].read_text())
    require(str(sbom.get("spdxVersion", "")).startswith("SPDX-2."), "invalid source SPDX SBOM")
    require(sbom.get("packages"), "empty source SBOM")
    new_output(out)
    for name, path in mapped.items():
        with path.open("rb") as source, (out / name).open("xb") as dest:
            shutil.copyfileobj(source, dest)
    (out / "release-notes.md").write_text(notes(version))
    assets = {p.name: dict(sha256=digest(p), size=p.stat().st_size) for p in sorted(out.iterdir())}
    manifest = dict(schema_version=1, version=version, source_commit=commit,
                    source_tag_object=run("git", "rev-parse", f"refs/tags/{version}") if tagged else None,
                    prerelease=True, targets=list(TARGETS), assets=assets)
    (out / "release-manifest.json").write_text(json.dumps(manifest, sort_keys=True, indent=2) + "\n")
    (out / "SHA256SUMS").write_text("".join(f"{digest(p)}  {p.name}\n" for p in sorted(out.iterdir())))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("check", "evidence", "source", "binary", "finalize"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--tagged", action="store_true")
    parser.add_argument("--out", type=Path)
    parser.add_argument("--target", choices=TARGETS)
    parser.add_argument("--binary-dir", type=Path)
    parser.add_argument("--incoming", type=Path)
    parser.add_argument("--checks", type=Path)
    parser.add_argument("--analyses", type=Path)
    parser.add_argument("--alerts", type=Path)
    args = parser.parse_args()
    if args.command == "check":
        print(json.dumps(dict(zip(("commit", "toolchain"), contract(args.version, args.tagged)))))
    elif args.command == "evidence":
        require(args.checks and args.analyses and args.alerts, "all three CI evidence files required")
        commit, _ = contract(args.version, args.tagged)
        # gh --paginate --slurp preserves every page, including late alerts.
        runs = [r for page in json.loads(args.checks.read_text()) for r in page["workflow_runs"]]
        analyses = [a for page in json.loads(args.analyses.read_text()) for a in page]
        alerts = [a for page in json.loads(args.alerts.read_text()) for a in page]
        verify_ci_evidence(commit, runs, analyses, alerts)
        print(json.dumps({"commit": commit, "ci_evidence": "verified"}))
    else:
        require(args.out is not None, "--out required")
        if args.command == "source":
            source_bundle(args.version, args.out, args.tagged)
        elif args.command == "binary":
            require(args.target and args.binary_dir, "--target and --binary-dir required")
            binary_bundle(args.version, args.target, args.binary_dir, args.out, args.tagged)
        else:
            require(args.incoming, "--incoming required")
            finalize(args.version, args.incoming, args.out, args.tagged)


if __name__ == "__main__":
    main()
