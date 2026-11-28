#!/usr/bin/env python3
"""Check changed Copier-managed settings against a canonical template render."""

import argparse
import json
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

import yaml

POLICY = ".github/repo-template-policy.json"
WORKFLOW = ".github/workflows/repo-template-policy.yml"
CHECKER = ".github/scripts/check-template-policy.py"
WORKFLOW_PATH = re.compile(r"\.github/workflows/[A-Za-z0-9][A-Za-z0-9_.-]*\.ya?ml\Z")
MISSING = object()
CONFIG = {
    "pyproject.toml": (
        ("tool", "ruff", "line-length"), ("tool", "ruff", "target-version"),
        ("tool", "ruff", "lint", "select"), ("tool", "mypy", "strict"),
        ("tool", "mypy", "python_version"),
        *(("tool", "taskipy", "tasks", key) for key in ("check", "fix", "test")),
    ),
    "Cargo.toml": (("package", "edition"), ("profile", "release", "lto"),
                   ("profile", "release", "strip")),
    "src-tauri/Cargo.toml": (("package", "edition"),),
    "rust-toolchain.toml": (("toolchain", "channel"), ("toolchain", "components")),
    "package.json": (
        *(("scripts", key) for key in ("check", "format", "format:check", "lint", "test",
                                       "test:run", "typecheck", "cargo:fmt", "cargo:clippy",
                                       "cargo:test")),
        *(("devDependencies", key) for key in ("@types/node", "oxlint", "oxlint-tsgolint", "prettier",
                                               "typescript", "vitest")),
    ),
}
EXACT = {CHECKER, WORKFLOW, ".node-version", ".python-version", ".oxlintrc.json", ".prettierrc.json"}
SUPPORTED_KEYS = {f"{name}:{'.'.join(key)}" for name, keys in CONFIG.items() for key in keys}
SOURCES = {
    "https://github.com/mizucopo/repo-template.git",
    "https://github.com/mizucopo/repo-template",
    "git@github.com:mizucopo/repo-template.git",
    "ssh://git@github.com/mizucopo/repo-template.git",
    "gh:mizucopo/repo-template",
}


def git(repo, *parts):
    return subprocess.check_output(["git", "-C", str(repo), *parts], timeout=60)


class Tree:
    """Read committed regular blobs without executing anything from the candidate."""

    def __init__(self, path):
        self.path = path
        self.files = {}
        for entry in git(path, "ls-tree", "-rz", "HEAD").split(b"\0"):
            if entry:
                info, name = entry.split(b"\t", 1)
                self.files[name.decode("utf-8", errors="surrogateescape")] = info.split()

    def read(self, name):
        if name not in self.files:
            return None
        mode, kind, object_id = self.files[name]
        if kind != b"blob" or mode not in {b"100644", b"100755"}:
            raise ValueError(f"managed path is not a regular file: {name!r}")
        oid = object_id.decode()
        if int(git(self.path, "cat-file", "-s", oid)) > 1024 * 1024:
            raise ValueError(f"managed file is too large: {name!r}")
        return git(self.path, "cat-file", "blob", oid)


class Render:
    def __init__(self, path):
        self.path = path
        self.files = {
            f.relative_to(path).as_posix() for name in ("workflows", "scripts")
            for f in (path / ".github" / name).rglob("*") if f.is_file()
        }

    def read(self, name):
        path = self.path / name
        return path.read_bytes() if path.is_file() else None


def policy(data):
    if data is None:
        raise ValueError(f"missing {POLICY}")
    value = json.loads(data)
    if not isinstance(value, dict) or set(value) != {"local_workflows", "local_managed_keys"}:
        raise ValueError("invalid policy schema")
    result = []
    for key, check in (("local_workflows", WORKFLOW_PATH.fullmatch),
                       ("local_managed_keys", SUPPORTED_KEYS.__contains__)):
        items = value[key]
        if not isinstance(items, list) or not all(
            isinstance(item, str) and check(item) for item in items
        ) or len(items) != len(set(items)):
            raise ValueError(f"invalid {key}; list unique exact supported paths, not globs")
        result.append(set(items))
    return result


def answers(tree):
    data = tree.read(".copier-answers.yml")
    value = yaml.safe_load(data) if data is not None else None
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise ValueError("invalid Copier answers")
    rev, src = value.get("_commit"), value.get("_src_path")
    if not isinstance(src, str) or src not in SOURCES:
        raise ValueError("Copier source differs from the canonical template repository")
    if not isinstance(rev, str) or not re.fullmatch(r"[0-9a-f]{7,40}", rev):
        raise ValueError("Copier revision must be a hexadecimal commit")
    if value.get("use_repo_template_policy") is not True:
        raise ValueError("the active repo-template policy cannot be disabled in a candidate PR")
    return rev, {key: v for key, v in value.items() if not key.startswith("_")}


def sha_at(repo, name):
    return git(repo, "rev-parse", "--verify", f"{name}^{{commit}}").decode().strip()


def ancestor(repo, earlier, later):
    run = subprocess.run(["git", "-C", str(repo), "merge-base", "--is-ancestor", earlier, later],
                         timeout=60, capture_output=True, check=False)
    if run.returncode:
        raise ValueError("template commit must advance on canonical main, not a separate branch")


def render(repo, rev, values, dest):
    answer_file = dest.parent / f"{dest.name}.json"
    answer_file.write_text(json.dumps(values, allow_nan=False), encoding="utf-8")
    run = subprocess.run(
        [sys.executable, "-m", "copier", "copy", "--trust", "--skip-tasks",
         "--defaults", "--vcs-ref", rev, "--data-file", str(answer_file), str(repo), str(dest)],
        timeout=180, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False,
    )
    if run.returncode:
        raise ValueError("canonical Copier render failed; check the approved answers and revision")
    return Render(dest)


def parsed(name, data):
    if data is None:
        return {}
    result = json.loads(data) if name.endswith(".json") else tomllib.loads(data.decode())
    if not isinstance(result, dict):
        raise ValueError(f"{name} is not structured configuration")
    return result


def field(value, key):
    for part in key:
        if not isinstance(value, dict) or part not in value:
            return MISSING
        value = value[part]
    return value


def differences(base, head, old, new, migration, rules):
    local_workflows, local_keys = rules
    failures = []
    for name in sorted(set(base.files) | set(head.files) | old.files | new.files | EXACT):
        is_workflow = (name.startswith(".github/workflows/") and
                       name.lower().endswith((".yml", ".yaml")))
        is_generated_script = name.startswith(".github/scripts/") and (
            name in old.files or name in new.files
        )
        if not is_workflow and not is_generated_script and name not in EXACT:
            continue
        before, after = base.read(name), head.read(name)
        if before == after:
            continue
        if is_workflow and not WORKFLOW_PATH.fullmatch(name) and after is not None:
            failures.append(repr(name))
            continue
        expected, expected_before = new.read(name), old.read(name)
        if name in {WORKFLOW, CHECKER} and after is None:
            failures.append(name)
        elif expected_before is not None and expected is None and migration.read(name) is not None:
            failures.append(name)
        elif expected is not None or expected_before is not None:
            if after != expected:
                failures.append(name)
        elif after is not None and name not in local_workflows and name not in EXACT:
            failures.append(name)
    for name, keys in CONFIG.items():
        before, after = base.read(name), head.read(name)
        if before == after:
            continue
        expected_before, expected = old.read(name), new.read(name)
        if expected_before is None and expected is None:
            continue
        a, b, c, d, m = [parsed(name, raw) for raw in (
            before, after, expected_before, expected, migration.read(name)
        )]
        for key in keys:
            label = f"{name}:{'.'.join(key)}"
            if label in local_keys or field(a, key) == field(b, key):
                continue
            old_value, expected_value = field(c, key), field(d, key)
            if old_value is MISSING and expected_value is MISSING:
                continue
            disabled = (old_value is not MISSING and expected_value is MISSING and
                        field(m, key) is not MISSING)
            if disabled or field(b, key) != expected_value:
                failures.append(label)
    return failures


def check(base_path, head_path, canonical):
    base, head = Tree(base_path), Tree(head_path)
    policy(head.read(POLICY))
    rules = policy(base.read(POLICY))
    before, old_values = answers(base)
    after, new_values = answers(head)
    old_sha, new_sha = sha_at(canonical, before), sha_at(canonical, after)
    ancestor(canonical, old_sha, new_sha)
    ancestor(canonical, new_sha, "HEAD")
    with tempfile.TemporaryDirectory(prefix="repo-template-policy-") as directory:
        root = Path(directory)
        old = render(canonical, old_sha, old_values, root / "base")
        new = old if old_sha == new_sha and old_values == new_values else render(
            canonical, new_sha, new_values, root / "candidate"
        )
        migration = old if old_sha == new_sha else (
            new if old_values == new_values else render(canonical, new_sha, old_values, root / "migration")
        )
        failures = differences(base, head, old, new, migration, rules)
    if failures:
        raise ValueError("unapproved local changes; update repo-template first:\n- " +
                         "\n- ".join(failures))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("base", "head", "template"):
        parser.add_argument(f"--{option}", type=Path, required=True)
    args = parser.parse_args()
    try:
        check(args.base, args.head, args.template)
    except (OSError, ValueError, TypeError, yaml.YAMLError, subprocess.SubprocessError) as exc:
        print(f"Template policy: {exc}", file=sys.stderr)
        return 1
    print("Template policy: no unapproved managed changes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
