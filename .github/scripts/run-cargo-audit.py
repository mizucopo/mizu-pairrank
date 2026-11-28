"""Run the Rust dependency gate and explain its blocking findings."""

import hashlib
import json
import os
import re
import subprocess  # nosec B404
import sys
import tomllib
from pathlib import Path


def denied_warning_kinds():
    # Match cargo-audit 0.22.2's config precedence and --no-yanked behavior.
    cargo_home = Path(os.environ.get("CARGO_HOME") or Path.home() / ".cargo")
    config = Path(".cargo/audit.toml")
    if not config.exists():
        config = cargo_home / "audit.toml"
    if not config.exists():
        return set()
    with config.open("rb") as stream:
        deny = set(tomllib.load(stream).get("output", {}).get("deny", []))
    kinds = {"unmaintained", "unsound"}
    return kinds if "warnings" in deny else deny & kinds


def identifier(value):
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_.+-]+", value):
        raise ValueError("invalid advisory or package identifier")
    return value


def findings(entries):
    if not isinstance(entries, list):
        raise ValueError("invalid findings list")
    return {
        (
            identifier(entry["package"]["name"]),
            identifier(entry["package"]["version"]),
            identifier(entry["advisory"]["id"]),
        )
        for entry in entries
    }


def run_scanner(binary, *options, lockfile=None):
    # Invoke the pinned scanner directly and pass the base lockfile as data.
    return subprocess.run(  # nosec B603
        [binary, "audit", "--no-yanked", *options],
        input=lockfile,
        capture_output=True,
        check=False,
    )


def log_scanner_output(*chunks):
    for line in b"\n".join(chunks).decode(errors="replace").splitlines():
        if line.strip():
            print(f"cargo-audit: {line}", file=sys.stderr)


def audit(binary, denied, *, base_lock=None):
    # Only the primary scan follows the configured fetch policy.
    file_option = ("--file", "Cargo.lock" if base_lock is None else "-")
    if base_lock is None:
        result = run_scanner(binary, "--json", *file_option)
    else:
        result = run_scanner(
            binary, "--json", *file_option, "--no-fetch", lockfile=base_lock
        )
    # The pinned scanner can print its own advisory diagnostics after the JSON report.
    payload, _, diagnostics = result.stdout.partition(b"\n")
    log_scanner_output(result.stderr, diagnostics)
    try:
        report = json.loads(payload)
        vulnerabilities = report["vulnerabilities"]
        blocked = findings(vulnerabilities["list"])
        if vulnerabilities["found"] is not bool(blocked):
            raise ValueError("inconsistent vulnerability report")
        warnings = set()
        for kind, entries in report["warnings"].items():
            affected = findings(entries)
            if kind in denied:
                blocked.update(affected)
            else:
                warnings.update(affected)
        if result.returncode != int(bool(blocked)):
            raise ValueError("report disagrees with scanner status")
        snapshot = (report["database"]["advisory-count"], report["settings"])
    except (AttributeError, KeyError, TypeError, ValueError) as error:
        raise ValueError(f"invalid audit report (exit {result.returncode})") from error
    # JSON skips cargo-audit's terminal dependency-tree validation. Keep the
    # configured output mode and failure checks using the same DB.
    gate = run_scanner(binary, *file_option, "--no-fetch", lockfile=base_lock)
    expected_code = int(bool(blocked))
    if base_lock is None or gate.returncode != expected_code:
        log_scanner_output(gate.stderr, gate.stdout)
    if gate.returncode != expected_code:
        raise ValueError(f"dependency gate disagrees with JSON report (exit {gate.returncode})")
    return blocked, warnings, snapshot


def base_findings(binary, denied, base_sha, lock_path, snapshot):
    if not re.fullmatch(r"[0-9a-f]{40}", base_sha):
        raise ValueError("base commit SHA unavailable")
    # Read the event's exact base commit without checking out or running its contents.
    result = subprocess.run(  # nosec B603, B607
        ["git", "show", f"{base_sha}:{lock_path}"],
        capture_output=True,
        check=False,
    )
    if result.returncode or not result.stdout:
        raise ValueError("base commit or committed Cargo.lock unavailable")
    blocked, _, base_snapshot = audit(binary, denied, base_lock=result.stdout)
    if base_snapshot != snapshot:
        raise ValueError("advisory database or audit settings changed")
    return blocked


def publish(lines):
    print("\n".join(lines))
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(summary).open("a", encoding="utf-8") as stream:
            stream.write("## Rust dependency audit\n\n" + "\n".join(lines) + "\n\n")


def main():
    binary, lock_path = sys.argv[1:]
    event = os.environ.get("GITHUB_EVENT_NAME")
    is_pr = event == "pull_request"
    try:
        # The audit works only on the project lockfile already tracked by Git.
        tracked = subprocess.run(  # nosec B603, B607
            ["git", "ls-files", "--error-unmatch", "Cargo.lock"],
            capture_output=True,
            check=False,
        )
        if tracked.returncode or not Path("Cargo.lock").stat().st_size:
            raise ValueError("a non-empty committed Cargo.lock is required")
        denied = denied_warning_kinds()
        blocked, warnings, snapshot = audit(binary, denied)
    except (OSError, ValueError) as error:
        print(f"Rust dependency audit failed: {error}", file=sys.stderr)
        if is_pr:
            publish(["未確認（PRの監査結果を取得できませんでした）"])
        return 1

    lines = []
    base = None
    if is_pr and blocked:
        try:
            base = base_findings(
                binary, denied, os.environ.get("AUDIT_BASE_SHA", ""), lock_path, snapshot
            )
        except (OSError, ValueError) as error:
            lines.append(f"未確認（ベースとの比較に失敗しました: {error}）\n")
    for finding in sorted(blocked):
        crate, version, advisory = finding
        if not is_pr:
            label = "ブロック"
        elif base is None:
            label = "未確認"
        else:
            label = "ベースにも存在" if finding in base else "PRで新規"
        lines.append(f"- {label} / {crate} {version} / {advisory}")
    if not blocked:
        lines.append("ブロック対象は検出されませんでした。\n")
    for crate, version, advisory in sorted(warnings):
        lines.append(f"- 警告 / {crate} {version} / {advisory}")
    publish(lines)
    if blocked and event == "schedule":
        fingerprint = hashlib.sha256(json.dumps(sorted(blocked)).encode("utf-8")).hexdigest()
        with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as stream:
            stream.write(f"fingerprint={fingerprint}\n")
    return int(bool(blocked))


if __name__ == "__main__":
    sys.exit(main())
