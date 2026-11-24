#!/usr/bin/env python3
"""Print the workspace packages a change touches, one per line.

Used by the GitHub Actions workflow to test only what changed: a file under
crates/<dir>/ names that crate's package; a change to the root Cargo.toml,
Cargo.lock, vendor/ or ci/ means everything ("ALL"). The tests crate is
never printed: its suites run on demand and before a release.

    ci/changed-crates.py BASE_COMMIT [HEAD]
"""
import re
import subprocess
import sys

base = sys.argv[1]
head = sys.argv[2] if len(sys.argv) > 2 else "HEAD"
files = subprocess.run(
    ["git", "diff", "--name-only", f"{base}...{head}"],
    check=True, capture_output=True, text=True,
).stdout.split()

packages = set()
for path in files:
    if path in ("Cargo.toml", "Cargo.lock") or path.startswith(("vendor/", "ci/")):
        print("ALL")
        sys.exit(0)
    match = re.match(r"crates/([^/]+)/", path)
    if not match:
        continue
    manifest = f"crates/{match.group(1)}/Cargo.toml"
    try:
        text = open(manifest, encoding="utf-8").read()
    except FileNotFoundError:
        continue
    name = re.search(r'^name\s*=\s*"([^"]+)"', text, re.M)
    if name:
        packages.add(name.group(1))
for package in sorted(packages):
    print(package)
