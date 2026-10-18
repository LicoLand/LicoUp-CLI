#!/usr/bin/env python3
"""Component-integration driver for V7-U1 (optional package lifecycle).

The packages this run installs are produced here, outside the Rust crate, by
this file alone: an ordinary package and its update, a package that ships an
install script, and an archive that tries to escape its package root. The
ignored black-box scenario in
``crates/licoup-native/src/platform/extension_packages/scenarios/external_fixtures.rs``
installs them on a real filesystem, and this driver then inspects the sandbox
after the Rust process has exited.

It writes only a temporary directory. It never touches a real installation, an
account, the network, the plan, the graph or the ledger.

Run from the repository root:

    python3 tests/integration/package_lifecycle/verify_component_lifecycle.py

``--keep`` leaves the temporary sandbox in place for inspection.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

sys.dont_write_bytecode = True

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
FILTER = "platform::extension_packages::scenarios::external_fixtures"
NET = "example.specialist/net"
ECHO = "example.specialist.echo"
SCRIPTED = "example.specialist.scripted"


def client_versions() -> list[str]:
    """The client versions these synthetic packages declare they support.

    The value comes from ``tools/client-version.json``, the same manifest the
    build injects into the binary under test, rather than a version restated
    here; the upper bound is the next major line, so the declaration is a real
    range and not an exact-version match.
    """
    published = json.loads((ROOT / "tools" / "client-version.json").read_text())
    version = published["productVersion"]
    next_major = int(version.split(".")[0]) + 1
    return [f">={version}, <{next_major}"]


def manifest(package_id: str, version: str) -> str:
    """A package manifest that satisfies the published extension schema."""
    return json.dumps(
        {
            "schema": "licoup.extension-package.v1",
            "id": package_id,
            "version": version,
            "displayName": "Synthetic specialist",
            "hostProtocol": {"major": 1, "minimumMinor": 0},
            "compatibility": {"clientVersions": client_versions()},
            "profiles": [{"id": "agent-execution", "major": 1}],
            "runtime": {"mode": "process", "entry": "agent.py"},
            "activation": "on-demand",
            "requires": [],
            "optionalRequires": [],
            "permissions": [{"capability": NET, "scope": "self"}],
            "contributions": [],
        }
    )


def write_package(path: Path, package_id: str, version: str, extra_files) -> None:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("manifest.json", manifest(package_id, version))
        archive.writestr("agent.py", "print('echo')\n")
        for name, content in extra_files:
            archive.writestr(name, content)


def make_fixtures(directory: Path, sandbox: Path) -> None:
    write_package(directory / "echo-1.0.0.zip", ECHO, "1.0.0", [])
    write_package(directory / "echo-1.1.0.zip", ECHO, "1.1.0", [])
    write_package(
        directory / "scripted-1.0.0.zip",
        SCRIPTED,
        "1.0.0",
        [
            (
                "postinstall.sh",
                "#!/bin/sh\n"
                f"echo ran > {sandbox / 'ran-script.txt'}\n",
            )
        ],
    )
    write_package(
        directory / "traversal-1.0.0.zip",
        "example.specialist.traversal",
        "1.0.0",
        [("../escape.txt", "escaped")],
    )


def run_scenario(fixtures: Path, sandbox: Path) -> subprocess.CompletedProcess:
    command = [
        "node",
        "tools/scripts/cargo-client.mjs",
        "test",
        "--manifest-path",
        "crates/licoup-native/Cargo.toml",
        "--lib",
        "--locked",
        FILTER,
        "--",
        "--ignored",
    ]
    environment = dict(
        os.environ,
        LICOUP_V71_PACKAGE_FIXTURES=str(fixtures),
        LICOUP_V71_PACKAGE_SANDBOX=str(sandbox),
    )
    return subprocess.run(
        command,
        cwd=ROOT,
        env=environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )


def passed_tests(output: str) -> int:
    return sum(
        int(count) for count in re.findall(r"test result: ok\. (\d+) passed;", output)
    )


def check_residue(sandbox: Path) -> list[str]:
    """Facts about the sandbox that the driver can see after the process ended."""
    failures: list[str] = []
    echo = sandbox / "packages" / ECHO
    if echo.exists():
        failures.append("the uninstalled package directory survived")
    scripted = sandbox / "packages" / SCRIPTED / "1.0.0" / "manifest.json"
    if not scripted.is_file():
        failures.append("the installed package's content is missing")
    staging = sandbox / "staging"
    if staging.is_dir() and any(staging.iterdir()):
        failures.append("staging still holds an abandoned directory")
    if (sandbox / "ran-script.txt").exists():
        failures.append("an install script was executed")
    if list(sandbox.rglob("escape.txt")):
        failures.append("an archive entry escaped its package root")
    journal = sandbox / "journal" / "install.jsonl"
    operations: list[str] = []
    if journal.is_file():
        for line in journal.read_text().splitlines():
            try:
                operations.append(str(json.loads(line).get("operation")))
            except json.JSONDecodeError:
                failures.append("the journal holds a line that is not an entry")
    else:
        failures.append("the install journal is missing")
    for expected in ("stage", "commit", "rollback", "uninstall"):
        if expected not in operations:
            failures.append(f"the journal has no {expected} entry")
    return failures


def main() -> int:
    keep = "--keep" in sys.argv[1:]
    temporary = Path(tempfile.mkdtemp(prefix="licoup-v71-pkg-"))
    fixtures = temporary / "fixtures"
    sandbox = temporary / "sandbox"
    fixtures.mkdir()
    sandbox.mkdir()
    try:
        make_fixtures(fixtures, sandbox)
        result = run_scenario(fixtures, sandbox)
        output = (
            result.stdout.replace(str(ROOT), "<repo>")
            .replace(str(temporary), "<sandbox>")
            .replace(str(Path.home()), "<home>")
        )
        count = passed_tests(output)

        failures: list[str] = []
        if result.returncode != 0:
            failures.append(f"the scenario command exited {result.returncode}")
        if count < 1:
            failures.append("the ignored scenario did not run")
        failures.extend(check_residue(sandbox))

        if failures:
            print(output.strip())
            for failure in failures:
                print(f"FAIL {failure}")
            print(f"status=failed scenarios_passed={count}")
            return 1
        print(f"status=passed scenarios_passed={count}")
        print(
            "observed: packages installed/uninstalled on disk, stage reclaimed, "
            "install script never run, traversal refused, journal recorded "
            "stage/commit/rollback/uninstall"
        )
        if keep:
            print(f"kept sandbox at {sandbox}")
        return 0
    finally:
        if not keep:
            shutil.rmtree(temporary, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
