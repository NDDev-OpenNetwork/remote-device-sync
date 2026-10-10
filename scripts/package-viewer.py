#!/usr/bin/env python3
"""Package the native Rust viewer as a macOS application, without host facts."""
import argparse
import os
import pathlib
import plistlib
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output already exists; choose a new application path")
    if not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error("binary must be an executable regular file")
    version = (pathlib.Path(__file__).resolve().parent.parent / "VERSION").read_text().strip()
    contents = args.output / "Contents"
    executable = contents / "MacOS" / "rds-viewer"
    executable.parent.mkdir(parents=True)
    shutil.copy2(args.binary, executable)
    executable.chmod(0o755)
    resources = contents / "Resources"
    resources.mkdir()
    icon = pathlib.Path(__file__).resolve().parent.parent / "crates/rds-desktop/assets/app-icon.icns"
    shutil.copy2(icon, resources / "RDS.icns")
    notices = pathlib.Path(__file__).resolve().parent.parent / "crates/rds-desktop/assets/font-licenses"
    shutil.copytree(notices, resources / "Font-Licenses")
    info = {
        "CFBundleName": "RDS",
        "CFBundleDisplayName": "RDS",
        "CFBundleIdentifier": "org.nddev.opennetwork.rds",
        "CFBundleExecutable": "rds-viewer",
        "CFBundleIconFile": "RDS.icns",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "LSMinimumSystemVersion": "12.0",
        "NSHighResolutionCapable": True,
    }
    with (contents / "Info.plist").open("wb") as stream:
        plistlib.dump(info, stream)
    subprocess.run(["codesign", "--force", "--sign", "-", str(args.output)], check=True)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(args.output)], check=True)
    print(args.output)


if __name__ == "__main__":
    main()
