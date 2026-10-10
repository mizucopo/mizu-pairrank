#!/usr/bin/env python3
"""Number main once, then build and publish that exact commit in the same run."""

import argparse
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path
from urllib import error, parse, request

POLICY = ".github/release.json"
LEVELS = {"patch", "minor", "major"}
SHA = re.compile(r"[0-9a-f]{40}\Z")
# The first nonnumeric prerelease character uniquely splits the identifier.
SEMVER = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-((?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?\Z"
)


class PreparationError(Exception):
    """A condition requires attention; never silently relax the contract."""


def require(condition, message):
    if not condition:
        raise PreparationError(message)


def canonical(value):
    return json.dumps(
        value, sort_keys=True, ensure_ascii=True, separators=(",", ":")
    ).encode()


def run(*args, data=None, env=None, cwd=None):
    result = subprocess.run(args, input=data, capture_output=True, cwd=cwd, env=env)
    require(
        result.returncode == 0,
        f"{args[0]} failed: {result.stderr.decode(errors='replace')[:1000]}",
    )
    return result.stdout


def output(name, value):
    value = str(value)
    require("\n" not in value and "\r" not in value, f"Invalid output: {name}")
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as stream:
            stream.write(f"{name}={value}\n")


def safe_path(path):
    require(
        isinstance(path, str)
        and path
        and not path.startswith(("/", "-"))
        and "\\" not in path
        and not any(c in path for c in "\r\n\0")
        and all(part not in {".", "..", ""} for part in path.split("/")),
        f"Invalid repository path: {path!r}",
    )
    return path


def version_key(version, scheme):
    if scheme == "chrome":
        require(
            isinstance(version, str)
            and re.fullmatch(r"(?:0|[1-9][0-9]*)(?:\.(?:0|[1-9][0-9]*)){0,3}", version),
            f"Invalid Chrome version: {version}",
        )
        parts = list(map(int, version.split(".")))
        require(
            any(parts) and max(parts) <= 65535,
            f"Chrome version out of range: {version}",
        )
        return tuple(parts + [0] * (4 - len(parts)))
    match = SEMVER.fullmatch(version) if isinstance(version, str) else None
    require(match, f"Invalid SemVer: {version}")
    core = tuple(map(int, match.groups()[:3]))
    pre = match.group(4)
    # Numeric identifiers precede nonnumeric identifiers; stable follows prerelease.
    if pre is None:
        return core, 1, ()
    identifiers = tuple(
        (0, int(item)) if item.isdigit() else (1, item) for item in pre.split(".")
    )
    return core, 0, identifiers


def bump(version, level, scheme):
    require(level in LEVELS, f"Unsupported release level: {level}")
    version_key(version, scheme)
    if scheme == "chrome":
        parts = list(map(int, version.split(".")))
        parts += [0] * max(0, 3 - len(parts))
        index = {"major": 0, "minor": 1, "patch": len(parts) - 1}[level]
        parts[index] += 1
        require(
            parts[index] <= 65535, f"Chrome {level} component exceeds 65535: {version}"
        )
        for i in range(index + 1, len(parts)):
            parts[i] = 0
        return ".".join(map(str, parts))
    match = SEMVER.fullmatch(version)
    parts = list(map(int, match.groups()[:3]))
    if match.group(4) and level == "patch":
        identifiers = match.group(4).split(".")
        require(
            identifiers[-1].isdigit(),
            "Prerelease needs an explicit numeric channel sequence",
        )
        identifiers[-1] = str(int(identifiers[-1]) + 1)
        return ".".join(map(str, parts)) + "-" + ".".join(identifiers)
    index = {"major": 0, "minor": 1, "patch": 2}[level]
    parts[index] = 1 if level == "major" and parts[0] == 0 else parts[index] + 1
    for i in range(index + 1, 3):
        parts[i] = 0
    return ".".join(map(str, parts))


def revision_number(value):
    require(
        isinstance(value, str) and re.fullmatch(r"r(?:0|[1-9][0-9]*)", value),
        f"Invalid revision: {value!r}; expected rN",
    )
    return int(value[1:])


def publication(policy, version, revision=None):
    if revision is not None:
        revision_number(revision)
    context = {
        "version": version,
        "revision": revision or "r0",
        "revision_suffix": "" if revision in (None, "r0") else f"-{revision}",
    }
    spec = policy["publication"]
    try:
        tag = spec["release_tag"].format_map(context)
        images = [
            # SemVer cannot contain '_'; preserve metadata without aliasing '-'.
            {**image, "tag": image["tag"].format_map(context).replace("+", "_")}
            for image in spec["images"]
        ]
    except (KeyError, ValueError) as exc:
        raise PreparationError(f"Invalid publication tag template: {exc}") from exc
    run("git", "check-ref-format", f"refs/tags/{tag}")
    for image in images:
        require(
            re.fullmatch(r"[a-z0-9][a-z0-9_-]*", image["name"])
            and image["name"] != "release_tag",
            f"Invalid/reserved publication setting name: {image['name']}",
        )
        require(
            re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}", image["tag"])
            and image["tag"] not in image.get("aliases", [])
            and image["tag"] != "latest",
            f"Invalid immutable image tag: {image['name']}",
        )
        run(
            "git",
            "check-ref-format",
            f"refs/heads/automation/docker-images/{image['tag']}",
        )
    require(
        len({(i["repository"], i["tag"]) for i in images}) == len(images),
        "Duplicate immutable image tags",
    )
    require(
        len({i["name"] for i in images}) == len(images),
        "Duplicate publication setting names",
    )
    require(
        spec.get("latest_image") is None
        or spec["latest_image"] in {i["name"] for i in images},
        "latest_image does not name a declared image",
    )
    return {
        "release_tag": tag,
        "images": images,
        "release_paths": spec["release_paths"],
        "latest_image": spec.get("latest_image"),
        "github_release": spec["github_release"],
        "is_prerelease": policy["version"]["scheme"] != "chrome"
        and version_key(version, "semver")[1] == 0,
    }


def choose(
    policy,
    base_version,
    level,
    explicit_minimum,
    occupied,
    base_revision=None,
    input_version=None,
    explicit_revision=None,
):
    scheme = policy["version"]["scheme"]
    if scheme == "upstream-revision":
        version = input_version or base_version
        precedence = version_key(version, "semver")
        base_precedence = version_key(base_version, "semver")
        require(precedence >= base_precedence, "Upstream version would decrease")
        floor = (
            revision_number(base_revision or "r0") + 1
            if precedence == base_precedence
            else 0
        )
        if explicit_revision is not None:
            require(
                revision_number(explicit_revision) >= floor,
                "Explicit revision does not meet required increment",
            )
            floor = revision_number(explicit_revision)
        revision = f"r{floor}"
    else:
        pre = SEMVER.fullmatch(base_version).group(4) if scheme == "semver" else None
        if pre and level != "patch":
            require(
                explicit_minimum is not None,
                "Prerelease core/stable transition needs an explicit version minimum",
            )
        if (
            pre
            and level == "patch"
            and not pre.split(".")[-1].isdigit()
            and explicit_minimum is not None
        ):
            # No automatic sequence exists; an explicit transition must exceed base.
            version = base_version
        else:
            version = bump(base_version, level, scheme)
        if explicit_minimum is not None:
            minimum_key = version_key(explicit_minimum, scheme)
            required_key = version_key(version, scheme)
            if scheme == "semver" and required_key[1] == 1:
                # A core increment may start a prerelease at that same core.
                # Existing prerelease patch sequences still use full precedence.
                minimum_key, required_key = minimum_key[0], required_key[0]
            require(
                minimum_key >= required_key
                and version_key(explicit_minimum, scheme)
                > version_key(base_version, scheme),
                f"Explicit minimum {explicit_minimum} does not meet {level} "
                f"increment from {base_version}",
            )
            version = explicit_minimum
        revision = None
    for _ in range(100):
        plan = publication(policy, version, revision)
        require(
            not plan["release_tag"].startswith("-"),
            "Release tag must not start with '-'",
        )
        collisions = occupied(plan)
        if not collisions:
            return version, revision, plan
        next_version, next_revision = (
            (version, f"r{revision_number(revision) + 1}")
            if revision is not None
            else (bump(version, "patch", scheme), None)
        )
        next_plan = publication(policy, next_version, next_revision)
        objects = {
            "release_tag": lambda p: p["release_tag"],
            **{
                image["name"]: lambda p, name=image["name"]: next(
                    i["tag"] for i in p["images"] if i["name"] == name
                )
                for image in plan["images"]
            },
        }
        for name in collisions:
            require(name in objects, f"Unknown collision target: {name}")
            require(
                objects[name](plan) != objects[name](next_plan),
                f"Publication setting {name} cannot advance its colliding tag",
            )
        version, revision = next_version, next_revision
    raise PreparationError(
        "No unused version after 100 candidates; inspect publication settings"
    )


def get_field(data, keys):
    for key in keys:
        require(
            isinstance(data, list) if isinstance(key, int) else isinstance(data, dict),
            "Version path container does not match its key",
        )
        data = data[key]
    return data


def read_field(blob, spec):
    text = blob.decode()
    if spec["format"] == "text":
        require(
            len(text.strip().splitlines()) == 1,
            f"Expected one version line: {spec['path']}",
        )
        return text.strip()
    data = json.loads(text) if spec["format"] == "json" else tomllib.loads(text)
    if spec["format"] == "toml-lock":
        packages = [
            p for p in data.get("package", []) if p.get("name") == spec["package"]
        ]
        require(
            len(packages) == 1, f"Missing or ambiguous root package in {spec['path']}"
        )
        return packages[0]["version"]
    return get_field(data, spec["key"])


def json_field_span(text, keys, index=0):
    """Locate one JSON value without changing whitespace or other fields."""
    decoder = json.JSONDecoder()

    def space(index):
        while index < len(text) and text[index].isspace():
            index += 1
        return index

    index = space(index)
    if not keys:
        _, end = decoder.raw_decode(text, index)
        return index, end
    opening = text[index]
    require(opening in {"{", "["}, "JSON version path is not a container")
    closing = "}" if opening == "{" else "]"
    index, ordinal = space(index + 1), 0
    while text[index] != closing:
        if opening == "{":
            key, index = decoder.raw_decode(text, index)
            index = space(index)
            require(text[index] == ":", "Invalid JSON property")
            index = space(index + 1)
        else:
            key = ordinal
        _, end = decoder.raw_decode(text, index)
        if key == keys[0]:
            return json_field_span(text, keys[1:], index)
        index = space(end)
        if text[index] != ",":
            break
        index, ordinal = space(index + 1), ordinal + 1
    raise PreparationError("JSON version field was not found")


def write_field(blob, spec, value):
    old = read_field(blob, spec)
    if old == value:
        return blob
    if spec["format"] == "text":
        return (value + "\n").encode()
    if spec["format"] == "json":
        text = blob.decode()
        start, end = json_field_span(text, spec["key"])
        updated = text[:start] + json.dumps(value, ensure_ascii=False) + text[end:]
        expected = json.loads(text)
        get_field(expected, spec["key"][:-1])[spec["key"][-1]] = value
        require(json.loads(updated) == expected, "Unexpected JSON version changes")
        return updated.encode()
    text = blob.decode()
    if spec["format"] == "toml-lock":
        chunks = re.split(r"(?=^\[\[package\]\]\s*$)", text, flags=re.M)
        candidates = [
            i
            for i, chunk in enumerate(chunks)
            if re.search(
                r'^name\s*=\s*"' + re.escape(spec["package"]) + r'"\s*$', chunk, re.M
            )
        ]
        require(len(candidates) == 1, f"Ambiguous lock package: {spec['path']}")
        i = candidates[0]
        chunks[i], count = re.subn(
            r'^version\s*=\s*"[^"\n]*"',
            f'version = "{value}"',
            chunks[i],
            count=1,
            flags=re.M,
        )
        text = "".join(chunks)
    else:
        section = ".".join(spec["key"][:-1])
        pattern = r"(^\[" + re.escape(section) + r"\][^\n]*\n)(.*?)(?=^\[|\Z)"
        match = re.search(pattern, text, re.M | re.S)
        require(match, f"Missing version table: {spec['path']}")
        body, count = re.subn(
            r"^" + re.escape(spec["key"][-1]) + r'\s*=\s*"[^"\n]*"',
            f'{spec["key"][-1]} = "{value}"',
            match.group(2),
            count=1,
            flags=re.M,
        )
        text = text[: match.start(2)] + body + text[match.end(2) :]
    require(
        count == 1 and read_field(text.encode(), spec) == value,
        f"Cannot safely update {spec['path']}",
    )
    # Parsing before/after and restoring the old value proves no other TOML data
    # changed.
    before, after = tomllib.loads(blob.decode()), tomllib.loads(text)
    if spec["format"] == "toml-lock":
        next(p for p in after["package"] if p["name"] == spec["package"])["version"] = (
            old
        )
    else:
        get_field(after, spec["key"][:-1])[spec["key"][-1]] = old
    require(before == after, f"Unexpected TOML changes in {spec['path']}")
    return text.encode()


class Git:
    def __init__(self, root="."):
        self.root = str(Path(root).resolve())

    def command(self, *args, data=None, env=None):
        return run(
            "git",
            "-c",
            "core.hooksPath=/dev/null",
            "-C",
            self.root,
            *args,
            data=data,
            env=env,
        )

    def text(self, *args):
        return self.command(*args).decode().strip()

    def entries(self, tree):
        result = {}
        for line in self.command("ls-tree", "-rz", tree).split(b"\0"):
            if not line:
                continue
            header, path = line.split(b"\t", 1)
            mode, kind, oid = header.decode().split()
            result[path.decode()] = (mode, kind, oid)
        return result

    def blob(self, tree, path, optional=False):
        entry = self.entries(tree).get(safe_path(path))
        if entry is None and optional:
            return None
        require(
            entry and entry[0] in {"100644", "100755"} and entry[1] == "blob",
            f"Missing regular data file: {path} at {tree}",
        )
        return self.command("cat-file", "blob", entry[2])

    def patch_tree(self, tree, changes, modes=None):
        entries = self.entries(tree)
        with tempfile.TemporaryDirectory() as directory:
            env = {**os.environ, "GIT_INDEX_FILE": str(Path(directory) / "index")}
            self.command("read-tree", tree, env=env)
            for path, data in changes.items():
                safe_path(path)
                if data is None:
                    self.command("update-index", "--force-remove", "--", path, env=env)
                else:
                    mode = (modes or {}).get(path, entries.get(path, ("100644",))[0])
                    require(
                        mode in {"100644", "100755"},
                        f"Cannot update nonregular file: {path}",
                    )
                    oid = (
                        self.command("hash-object", "-w", "--stdin", data=data)
                        .decode()
                        .strip()
                    )
                    self.command(
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        f"{mode},{oid},{path}",
                        env=env,
                    )
            return self.command("write-tree", env=env).decode().strip()

    def fetch_main(self):
        self.command("fetch", "origin", "main", "--tags")
        return self.text("rev-parse", "FETCH_HEAD")

    def ancestors(self, commit):
        return self.text("rev-list", "--first-parent", commit).splitlines()

    def commit(self, tree, parent, message):
        return (
            self.command("commit-tree", tree, "-p", parent, data=message.encode())
            .decode()
            .strip()
        )

    def tag_object(self, commit, tag):
        ident = self.text("var", "GIT_COMMITTER_IDENT")
        data = (
            f"object {commit}\ntype commit\ntag {tag}\n"
            f"tagger {ident}\n\nRelease {tag}\n"
        )
        return self.command("mktag", data=data.encode()).decode().strip()


def read_version(git, tree, policy, allow_missing=False):
    specs = policy["version"]["sources"]
    require(specs, "No version sources declared")
    scheme = policy["version"]["scheme"]
    scheme = "semver" if scheme == "upstream-revision" else scheme
    values = []
    for spec in specs:
        blob = git.blob(tree, spec["path"], allow_missing)
        if blob is None:
            continue
        try:
            value = read_field(blob, spec)
        except (
            KeyError,
            IndexError,
        ):
            if not allow_missing:
                raise
            continue
        if allow_missing:
            # A new field may be absent; existing fields must still be valid.
            version_key(value, scheme)
        values.append(value)
    require(len(set(values)) <= 1, "Declared version sources disagree")
    if len(values) != len(specs):
        return None
    version_key(values[0], scheme)
    return values[0]


def update_versions(git, tree, policy, version, revision):
    changes = {}
    for spec in policy["version"]["sources"] + policy["version"].get("locks", []):
        blob = changes.get(
            spec["path"], git.blob(tree, spec["path"], spec.get("optional", False))
        )
        if blob is not None:
            changes[spec["path"]] = write_field(blob, spec, version)
    if revision is not None:
        changes[policy["version"]["revision_path"]] = (revision + "\n").encode()
    return git.patch_tree(tree, changes)


class GitHub:
    def __init__(self, repository=None):
        self.repository = repository or os.environ["GITHUB_REPOSITORY"]
        self.root = os.environ.get("GITHUB_API_URL", "https://api.github.com")

    def api(self, path, method="GET", payload=None, missing=False, raw=False):
        url = (
            os.environ.get("GITHUB_GRAPHQL_URL", self.root + path)
            if path == "/graphql"
            else self.root + path
        )
        headers = {
            "Authorization": f"Bearer {os.environ['GH_TOKEN']}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2026-03-10",
            "Content-Type": "application/json",
        }
        data = canonical(payload) if payload is not None else None
        req = request.Request(url, headers=headers, data=data, method=method)
        try:
            with request.urlopen(req, timeout=30) as response:
                body = response.read()
        except error.HTTPError as exc:
            if missing and exc.code == 404:
                return None
            raise PreparationError(
                f"GitHub API {method} {path}: HTTP {exc.code}; "
                "check token and permissions"
            ) from exc
        except error.URLError as exc:
            raise PreparationError(f"GitHub API unavailable: {exc.reason}") from exc
        if raw:
            return body.decode()
        if not body:
            return None
        return json.loads(body)

    def repo(self, path="", **kwargs):
        return self.api(f"/repos/{self.repository}{path}", **kwargs)

    def pages(self, path, key=None):
        result = []
        for page in range(1, 1001):
            value = self.repo(
                f"{path}{'&' if '?' in path else '?'}per_page=100&page={page}"
            )
            rows = value[key] if key else value
            result.extend(rows)
            if len(rows) < 100:
                return result
        raise PreparationError("GitHub pagination exceeded 1000 pages")

    def commit_prs(self, commits):
        for _, candidates in self.commit_associations(commits):
            yield from candidates

    def commit_associations(self, commits):
        owner, name = self.repository.split("/")
        for start in range(0, len(commits), 50):
            batch = commits[start : start + 50]
            associations = {sha: [] for sha in batch}
            pending = {f"c{index}": (sha, None) for index, sha in enumerate(batch)}
            for _ in range(1000):
                declarations = ["$owner:String!", "$name:String!"]
                variables = {"owner": owner, "name": name}
                fields = []
                for alias, (sha, cursor) in pending.items():
                    declarations.extend(
                        [f"${alias}:GitObjectID!", f"${alias}After:String"]
                    )
                    variables.update({alias: sha, alias + "After": cursor})
                    fields.append(
                        f"{alias}:object(oid:${alias}) "
                        "{ ... on Commit { "
                        f"associatedPullRequests(first:100,after:${alias}After) "
                        "{ nodes { number merged baseRefName } "
                        "pageInfo { hasNextPage endCursor } } } }"
                    )
                query = (
                    f"query({','.join(declarations)}) "
                    "{ repository(owner:$owner,name:$name) { "
                    + " ".join(fields)
                    + " } }"
                )
                result = self.api(
                    "/graphql",
                    method="POST",
                    payload={
                        "query": query,
                        "variables": variables,
                    },
                )
                require(
                    isinstance(result, dict) and not result.get("errors"),
                    "GitHub commit association query failed",
                )
                data = result.get("data")
                repository = data.get("repository") if isinstance(data, dict) else None
                require(
                    isinstance(repository, dict) and repository,
                    "GitHub commit association repository unavailable",
                )
                following = {}
                for alias, (sha, cursor) in pending.items():
                    commit = repository.get(alias)
                    connection = (
                        commit.get("associatedPullRequests")
                        if isinstance(commit, dict)
                        else None
                    )
                    require(
                        isinstance(connection, dict),
                        f"Missing PR association evidence for {sha}",
                    )
                    nodes, page = connection.get("nodes"), connection.get("pageInfo")
                    require(
                        isinstance(nodes, list)
                        and all(
                            isinstance(node, dict)
                            and type(node.get("number")) is int
                            and node["number"] > 0
                            and type(node.get("merged")) is bool
                            and isinstance(node.get("baseRefName"), str)
                            for node in nodes
                        )
                        and isinstance(page, dict)
                        and type(page.get("hasNextPage")) is bool
                        and "endCursor" in page,
                        f"Incomplete PR association evidence for {sha}",
                    )
                    associations[sha].extend(nodes)
                    if page["hasNextPage"]:
                        after = page.get("endCursor")
                        require(
                            isinstance(after, str) and after and after != cursor,
                            "Invalid PR association cursor",
                        )
                        following[alias] = (sha, after)
                pending = following
                if not pending:
                    break
            else:
                raise PreparationError(
                    "GitHub PR association pagination exceeded 1000 pages"
                )
            # Finish every page before selecting a PR. Preserve main ancestry
            # order even when older commits need additional association pages.
            for sha in batch:
                yield sha, associations[sha]


def policy_at(git, commit):
    policy = json.loads(git.blob(commit, POLICY))
    require(
        set(policy) == {"version", "publication"},
        "release.json requires version and publication only",
    )
    return policy


def classification(pr):
    labels = [
        label["name"] for label in pr["labels"] if label["name"].startswith("release:")
    ]
    if not labels:
        return None
    require(
        len(labels) == 1 and labels[0][8:] in LEVELS,
        f"PR #{pr['number']} needs exactly one release:patch/minor/major label",
    )
    require(
        bool(re.sub(r"<!--.*?(?:-->|$)", "", pr.get("body") or "", flags=re.S).strip()),
        f"PR #{pr['number']} needs a classification reason in its body",
    )
    return labels[0][8:]


def record_at(git, commit):
    message = git.text("show", "-s", "--format=%B", commit)
    lines = message.splitlines()
    # A marker quoted in ordinary prose is not a record. Once release fields
    # appear, validate the entire record, including missing or invalid fields.
    if "Repo-Template-Release: 1" not in lines or not any(
        line.startswith("Release-") for line in lines
    ):
        return None
    fields = {}
    for line in lines:
        if line.startswith("Release-"):
            key, sep, value = line.partition(":")
            require(sep and key[8:] not in fields, "Invalid release commit trailers")
            fields[key[8:]] = value.strip()
    require(
        set(fields) == {"Source", "Run", "Tag", "Version", "PRs"},
        "Invalid release commit record",
    )
    require(
        SHA.fullmatch(fields["Source"]) and fields["Run"].isdigit(),
        "Invalid release source/run",
    )
    require(
        git.text("rev-list", "--parents", "-n", "1", commit).split()
        == [commit, fields["Source"]],
        "Release source must be the sole parent",
    )
    policy = policy_at(git, commit)
    require(
        read_version(git, commit, policy) == fields["Version"],
        "Release version differs from its record",
    )
    plan = publication_at(git, commit, policy)
    require(plan["release_tag"] == fields["Tag"], "Release tag differs from its record")
    allowed = {
        spec["path"]
        for spec in policy["version"]["sources"] + policy["version"].get("locks", [])
    }
    if policy["version"].get("revision_path"):
        allowed.add(policy["version"]["revision_path"])
    changed = set(
        git.text(
            "diff-tree", "--no-commit-id", "--name-only", "-r", commit
        ).splitlines()
    )
    require(changed <= allowed, "Release commit changes files outside version metadata")
    fields["commit"] = commit
    return fields


def records(git, main):
    commits = git.text(
        "log",
        "--first-parent",
        "--format=%H",
        "--grep=^Repo-Template-Release: 1$",
        main,
    )
    return [
        record
        for commit in commits.splitlines()
        if (record := record_at(git, commit)) is not None
    ]


def publication_at(git, commit, policy=None):
    policy = policy or policy_at(git, commit)
    version = read_version(git, commit, policy)
    revision_path = policy["version"].get("revision_path")
    revision = (
        (git.blob(commit, revision_path, True) or b"r0").decode().strip()
        if revision_path
        else None
    )
    return publication(policy, version, revision)


def registry_json(url, headers=None, payload=None, missing=False):
    headers = dict(headers or {})
    data = canonical(payload) if payload is not None else None
    if data is not None:
        headers["Content-Type"] = "application/json"
    try:
        with request.urlopen(
            request.Request(url, data=data, headers=headers), timeout=30
        ) as response:
            return json.load(response)
    except error.HTTPError as exc:
        if missing and exc.code == 404:
            return None
        raise PreparationError(
            f"Registry lookup failed: HTTP {exc.code} at {url}"
        ) from exc
    except (error.URLError, ValueError) as exc:
        raise PreparationError(f"Registry lookup failed at {url}") from exc


def image_digest(image):
    if image.get("registry", "dockerhub") == "ecr":
        result = subprocess.run(
            [
                "aws",
                "ecr",
                "describe-images",
                "--registry-id",
                image["account"],
                "--region",
                image["region"],
                "--repository-name",
                image["repository"],
                "--image-ids",
                f"imageTag={image['tag']}",
            ],
            capture_output=True,
        )
        if result.returncode:
            require(
                b"ImageNotFoundException" in result.stderr,
                f"ECR lookup failed for {image['name']}",
            )
            return None
        rows = json.loads(result.stdout)["imageDetails"]
        require(len(rows) == 1, "ECR returned ambiguous image state")
        digests = [rows[0].get("imageDigest")]
    else:
        headers = {}
        if os.environ.get("DOCKERHUB_TOKEN"):
            auth = registry_json(
                "https://hub.docker.com/v2/auth/token",
                payload={
                    "identifier": os.environ["DOCKERHUB_USERNAME"],
                    "secret": os.environ["DOCKERHUB_TOKEN"],
                },
            )
            token = auth.get("access_token")
            require(
                isinstance(token, str) and token,
                "Docker Hub did not return an access token",
            )
            headers["Authorization"] = "Bearer " + token
        root = f"https://hub.docker.com/v2/repositories/{image['repository']}/"
        registry_json(
            root, headers
        )  # A private/unknown repository is not an absent tag.
        data = registry_json(
            root + "tags/" + parse.quote(image["tag"], safe="") + "/",
            headers,
            missing=True,
        )
        if data is None:
            return None
        digests = (
            [data["digest"]]
            if "digest" in data
            else [row.get("digest") for row in data.get("images", [])]
        )
    require(
        digests
        and all(
            isinstance(d, str) and re.fullmatch(r"sha256:[0-9a-f]{64}", d)
            for d in digests
        ),
        f"Registry returned no verifiable digest for {image['name']}",
    )
    return "|".join(sorted(set(digests)))


def remote_collisions(gh, plan):
    tag = parse.quote(plan["release_tag"], safe="")
    # Authenticated listing includes drafts; the tag endpoint alone does not.
    collisions = (
        ["release_tag"]
        if (
            gh.repo(f"/git/ref/tags/{tag}", missing=True) is not None
            or any(r["tag_name"] == plan["release_tag"] for r in gh.pages("/releases"))
        )
        else []
    )
    collisions.extend(
        image["name"] for image in plan["images"] if image_digest(image) is not None
    )
    return collisions


def prepared(git, record):
    require(not record["Tag"].startswith("-"), "Release tag must not start with '-'")
    require(
        git.text("rev-parse", "refs/tags/" + record["Tag"] + "^{commit}")
        == record["commit"],
        "Reserved release tag does not point to its numbering commit",
    )
    return {
        "publish": "true",
        "release_sha": record["commit"],
        "release_tag": record["Tag"],
        "version": record["Version"],
    }


def verify_squash_merge(git, gh, commit, pr):
    parents = git.text("rev-list", "--parents", "-n", "1", commit).split()
    require(len(parents) == 2, "Use squash merge only")
    base = pr.get("base", {}).get("sha")
    history = git.ancestors(commit)
    require(
        isinstance(base, str) and SHA.fullmatch(base) and base in history[1:],
        f"PR #{pr['number']} needs a verifiable main base SHA",
    )
    # The recorded base can predate unrelated main updates. Only commits
    # introduced by this PR after that base distinguish a multi-commit rebase.
    # Associations at or before the base are not evidence of this merge.
    require(
        not any(
            candidate["number"] == pr["number"]
            and candidate["merged"]
            and candidate["baseRefName"] == "main"
            for candidate in gh.commit_prs(history[1 : history.index(base)])
        ),
        f"Use squash merge only: PR #{pr['number']} introduced multiple main commits",
    )


def preparation_input(git, gh, run_id, control_commit=None):
    require(str(run_id).isdigit(), "GITHUB_RUN_ID is required")
    require(
        gh.repo()["default_branch"] == "main", "Release requires default branch main"
    )
    main = git.fetch_main()
    history = records(git, main)
    prior = next((r for r in history if r["Run"] == str(run_id)), None)
    if prior:
        return {"record": prior}
    if control_commit:

        def controls(commit):
            return {
                path: entry
                for path, entry in git.entries(commit).items()
                if path == POLICY
                or path.startswith((".github/scripts/", ".github/workflows/"))
            }

        if controls(control_commit) != controls(main):
            # The newer push has its own queued run with the updated workflow.
            return None
    last = history[0] if history else None
    introduction = None
    if last:
        boundary = last["commit"]
    else:
        introductions = git.text(
            "log",
            "--first-parent",
            "--diff-filter=A",
            "--format=%H",
            main,
            "--",
            POLICY,
        ).splitlines()
        require(introductions, "Cannot find release configuration introduction")
        introduction = introductions[-1]
        parents = git.text("rev-list", "--parents", "-n", "1", introduction).split()
        boundary = parents[1] if len(parents) == 2 else None
    commits = git.ancestors(main)
    pending = commits[: commits.index(boundary)] if boundary else commits
    if not pending:
        return None
    # Stop at the newest merge on main; older PR classifications are irrelevant.
    pr = None
    for commit, candidates in gh.commit_associations(pending):
        # API 2026-03-10 omits merge_commit_sha from PRs. The timeline's
        # merged event still identifies the final merge commit.
        matches = []
        for candidate in candidates:
            if not candidate["merged"] or candidate["baseRefName"] != "main":
                continue
            merges = [
                event.get("commit_id")
                for event in gh.pages(f"/issues/{candidate['number']}/timeline")
                if event.get("event") == "merged"
            ]
            require(
                len(merges) == 1
                and isinstance(merges[0], str)
                and SHA.fullmatch(merges[0]),
                f"PR #{candidate['number']} needs a verifiable merge event",
            )
            if merges[0] == commit:
                matches.append(candidate)
        require(
            len(matches) <= 1, f"Commit {commit} needs exactly one squash-merged PR"
        )
        if matches:
            pr = gh.repo(f"/pulls/{matches[0]['number']}")
            verify_squash_merge(git, gh, commit, pr)
            break
    if pr is None:
        return None
    level = classification(pr)
    if level is None:
        return None
    return {
        "main": main,
        "last": last,
        "baseline": last["commit"] if last else boundary or commits[-1],
        "introduction": introduction,
        "pr": pr,
        "level": level,
    }


def intent(git, gh, run_id, control_commit=None):
    candidate = preparation_input(git, gh, run_id, control_commit)
    if candidate is None:
        return {"publish": "false"}
    if "record" in candidate:
        return prepared(git, candidate["record"])
    return {"publish": "true"}


def prepare(git, gh, run_id, control_commit=None):
    for _ in range(3):
        candidate = preparation_input(git, gh, run_id, control_commit)
        if candidate is None:
            return {"publish": "false"}
        if "record" in candidate:
            return prepared(git, candidate["record"])
        main, last = candidate["main"], candidate["last"]
        pr, level = candidate["pr"], candidate["level"]
        numbers = [pr["number"]]
        policy = policy_at(git, main)
        version = read_version(git, main, policy)
        revision_path = policy["version"].get("revision_path")
        revision = (
            (git.blob(main, revision_path, True) or b"r0").decode().strip()
            if revision_path
            else None
        )
        baseline = candidate["baseline"]
        base_version = (
            read_version(git, baseline, policy, allow_missing=not last)
            if baseline
            else None
        )
        if base_version is None:
            # A newly introduced source has no parent value. Preserve its
            # adoption value even when unlabeled PRs defer the first release.
            baseline = candidate["introduction"]
            base_version = read_version(git, baseline, policy_at(git, baseline))
        base_revision = (
            (git.blob(baseline, revision_path, True) or b"r0").decode().strip()
            if baseline and revision_path
            else revision
        )
        explicit = version if version != base_version and not revision_path else None
        version, revision, plan = choose(
            policy,
            base_version,
            level,
            explicit,
            lambda p: remote_collisions(gh, p),
            base_revision=base_revision,
            input_version=version,
            explicit_revision=revision
            if revision_path and revision != base_revision
            else None,
        )
        tree = update_versions(git, main, policy, version, revision)
        message = (
            f"chore(release): {plan['release_tag']}\n\nRepo-Template-Release: 1\n"
            f"Release-Source: {main}\nRelease-Run: {run_id}\n"
            f"Release-Tag: {plan['release_tag']}\n"
            f"Release-Version: {version}\nRelease-PRs: {','.join(map(str, numbers))}\n"
        )
        commit = git.commit(tree, main, message)
        tag = git.tag_object(commit, plan["release_tag"])
        try:
            git.command(
                "push",
                "--atomic",
                "origin",
                f"{commit}:refs/heads/main",
                f"{tag}:refs/tags/{plan['release_tag']}",
            )
        except PreparationError:
            # A timeout may have followed a successful push. Verify before retrying.
            current = git.fetch_main()
            saved = next(
                (r for r in records(git, current) if r["Run"] == str(run_id)), None
            )
            if saved:
                return prepared(git, saved)
            require(
                current != main or remote_collisions(gh, plan),
                "Atomic release push rejected; check token permissions, "
                "branch/tag rules and remote state",
            )
            continue
        git.fetch_main()
        return prepared(git, record_at(git, commit))
    raise PreparationError("main changed during all three numbering attempts; rerun")


def checkout_plan(git):
    commit = git.text("rev-parse", "HEAD")
    require(
        commit == os.environ.get("RELEASE_SHA"),
        "Checkout must match the prepared release SHA",
    )
    record = record_at(git, commit)
    require(record, "Checkout is not a numbering commit")
    prepared(git, record)
    return {
        "publication": publication_at(git, commit),
        "generated_version": record["Version"],
    }


DOWNLOADS_START = "<!-- repo-template:downloads:start -->"
DOWNLOADS_END = "<!-- repo-template:downloads:end -->"


def checked_release_id(release, tag):
    release_id = release.get("id")
    require(
        type(release_id) is int
        and release_id > 0
        and release.get("tag_name") == tag
        and type(release.get("draft")) is bool,
        "Invalid GitHub Release identity",
    )
    return release_id


def create_release(gh, plan):
    tag = plan["release_tag"]
    matches = [r for r in gh.pages("/releases") if r["tag_name"] == tag]
    require(len(matches) <= 1, "Multiple GitHub Releases match the tag")
    if matches:
        return checked_release_id(matches[0], tag)
    require(
        gh.repo(f"/git/ref/tags/{parse.quote(tag, safe='')}", missing=True),
        "Git tag must exist before creating a GitHub Release",
    )
    notes_path = os.environ.get("RELEASE_NOTES_PATH")
    if notes_path:
        body = Path(notes_path).read_text()
    else:
        body = gh.repo(
            "/releases/generate-notes", method="POST", payload={"tag_name": tag}
        )["body"]
    draft = os.environ.get("RELEASE_DRAFT") == "true"
    release = gh.repo(
        "/releases",
        method="POST",
        payload={
            "tag_name": tag,
            "name": tag,
            "body": body,
            "draft": draft,
            "prerelease": plan["is_prerelease"],
            "make_latest": "false",
        },
    )
    release_id = checked_release_id(release, tag)
    require(
        release["draft"] == draft
        and type(release.get("prerelease")) is bool
        and release["prerelease"] == plan["is_prerelease"],
        "Created GitHub Release metadata differs from publication plan",
    )
    return release_id


def distribution_body(body, assets, images):
    lines = []
    downloads = [
        asset
        for asset in assets
        if asset.get("state") == "uploaded" and asset.get("size", 0) > 0
    ]
    if downloads:
        lines = ["## Downloads", ""]
        for asset in downloads:
            name = asset["name"].replace("[", r"\[").replace("]", r"\]")
            lines.append(f"- [{name}](<{asset['browser_download_url']}>)")
    if images:
        lines += ["", "## Docker Images", ""]
        for image in images:
            repository = image["repository"]
            if image.get("registry", "dockerhub") == "ecr":
                repository = (
                    f"{image['account']}.dkr.ecr.{image['region']}.amazonaws.com/"
                    + repository
                )
            else:
                lines.append(f"- [Docker Hub](https://hub.docker.com/r/{repository})")
            lines += ["", "```sh", f"docker pull {repository}:{image['tag']}", "```"]
    newline = "\r\n" if "\r\n" in body else "\n"
    block = (
        DOWNLOADS_START
        + newline
        + newline.join(lines).strip()
        + newline
        + DOWNLOADS_END
        if lines
        else ""
    )
    pattern = re.compile(
        "^"
        + re.escape(DOWNLOADS_START)
        + r"\r?$.*?^"
        + re.escape(DOWNLOADS_END)
        + r"(?=\r?$)",
        re.MULTILINE | re.DOTALL,
    )
    if DOWNLOADS_START in body or DOWNLOADS_END in body:
        require(
            body.count(DOWNLOADS_START) == body.count(DOWNLOADS_END) == 1
            and len(pattern.findall(body)) == 1,
            "Malformed release downloads block",
        )
        return pattern.sub(lambda _: block, body)
    return body + (newline * 2 if body and block else "") + block


def rust_asset(git, gh, release, tag, assets):
    root = Path(git.root)
    package = tomllib.loads((root / "Cargo.toml").read_text())["package"]["name"]
    metadata = json.loads(
        run("cargo", "metadata", "--no-deps", "--format-version=1", cwd=git.root)
    )
    targets = next(p["targets"] for p in metadata["packages"] if p["name"] == package)
    if not any("bin" in target["kind"] for target in targets):
        return  # A library has no executable to distribute.
    name = f"{package}-{parse.quote(tag, safe='')}-linux-x86_64.tar.gz"
    if any(
        a["name"] == name and a.get("state") == "uploaded" and a.get("size", 0) > 0
        for a in assets
    ):
        return
    build = run(
        "cargo",
        "build",
        "--release",
        "--locked",
        "--package",
        package,
        "--bins",
        "--target",
        "x86_64-unknown-linux-gnu",
        "--message-format=json",
        cwd=git.root,
    )
    binaries = []
    for line in build.decode().splitlines():
        artifact = json.loads(line)
        if (
            artifact.get("reason") == "compiler-artifact"
            and "bin" in artifact["target"]["kind"]
            and artifact.get("executable")
        ):
            binaries.append(root / artifact["executable"])
    if not binaries:
        return  # All executable targets may require disabled optional features.
    require(release["draft"], "Published Rust distribution asset is missing")
    with tempfile.TemporaryDirectory() as directory:
        archive = Path(directory) / name
        with tarfile.open(archive, "w:gz") as bundle:
            for binary in binaries:
                bundle.add(binary, arcname=binary.name)
        incomplete = [asset for asset in assets if asset["name"] == name]
        require(
            all(type(a.get("id")) is int and a["id"] > 0 for a in incomplete),
            "Invalid release asset ID",
        )
        upload_url = release.get("upload_url")
        require(
            isinstance(upload_url, str) and upload_url, "Invalid release upload URL"
        )
        upload_url = upload_url.split("{")[0]
        for asset in incomplete:
            gh.repo(f"/releases/assets/{asset['id']}", method="DELETE")
        run(
            "gh",
            "api",
            "--method",
            "POST",
            f"{upload_url}?name={parse.quote(name, safe='')}",
            "-H",
            "Content-Type: application/gzip",
            "--input",
            str(archive),
            cwd=git.root,
        )
    uploaded = gh.pages(f"/releases/{release['id']}/assets")
    require(
        any(
            a["name"] == name and a.get("state") == "uploaded" and a.get("size", 0) > 0
            for a in uploaded
        ),
        "Rust distribution upload is incomplete",
    )


def distribution(git, gh, plan):
    tag = plan["release_tag"]
    matches = [r for r in gh.pages("/releases") if r["tag_name"] == tag]
    retained_id = os.environ.get("RELEASE_ID")
    require(len(matches) <= 1, "Multiple GitHub Releases match the tag")
    if retained_id:
        require(
            re.fullmatch(r"[1-9][0-9]*", retained_id),
            "Invalid retained GitHub Release ID",
        )
        release_id = int(retained_id)
        require(
            not matches or checked_release_id(matches[0], tag) == release_id,
            "Listed GitHub Release differs from retained ID",
        )
    else:
        require(len(matches) == 1, "Expected one GitHub Release for distribution")
        release_id = checked_release_id(matches[0], tag)
    release = gh.repo(f"/releases/{release_id}")
    require(checked_release_id(release, tag) == release_id, "GitHub Release ID differs")
    assets = gh.pages(f"/releases/{release['id']}/assets")
    if os.environ.get("BUILD_RUST_BINARY") == "true":
        rust_asset(git, gh, release, tag, assets)
        assets = gh.pages(f"/releases/{release['id']}/assets")
    for image in plan["images"]:
        require(
            image_digest(image) is not None,
            f"Published image is missing: {image['name']}",
        )
    release = gh.repo(f"/releases/{release['id']}")
    require(checked_release_id(release, tag) == release_id, "GitHub Release ID differs")
    body = release.get("body") or ""
    updated = distribution_body(body, assets, plan["images"])
    payload = {}
    if updated != body:
        payload["body"] = updated
    if release["draft"]:
        payload.update(
            draft=False, prerelease=plan["is_prerelease"], make_latest="false"
        )
    if payload:
        gh.repo(f"/releases/{release['id']}", method="PATCH", payload=payload)


def latest(git, gh, commit):
    if publication_at(git, commit)["is_prerelease"]:
        return False
    completed = [
        r for r in gh.pages("/releases") if not r["draft"] and not r.get("prerelease")
    ]
    require(
        len({r["tag_name"] for r in completed}) == len(completed),
        "Ambiguous completed releases",
    )
    releases = {r["tag_name"]: r for r in completed}
    for record in records(git, git.fetch_main()):
        if (
            record["Tag"] in releases
            and not publication_at(git, record["commit"])["is_prerelease"]
        ):
            prepared(git, record)
            return record["commit"] == commit
    raise PreparationError("No completed release exists on main")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "action",
        choices=[
            "intent",
            "prepare",
            "plan",
            "latest",
            "image-state",
            "classification",
            "distribution",
            "create",
        ],
    )
    parser.add_argument("--root", default=".")
    args = parser.parse_args()
    git, gh = Git(args.root), GitHub()
    if args.action == "classification":
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
        classification(gh.repo(f"/pulls/{event['number']}"))
        return
    require(os.environ.get("GITHUB_REF") == "refs/heads/main", "Release requires main")
    if args.action in {"intent", "prepare"}:
        action = intent if args.action == "intent" else prepare
        values = action(git, gh, os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_SHA"])
    else:
        plan = checkout_plan(git)
        if args.action == "plan":
            Path(os.environ["RELEASE_PLAN_OUTPUT"]).write_bytes(canonical(plan))
            values = {"version": plan["generated_version"]}
        elif args.action == "create":
            values = {"release_id": create_release(gh, plan["publication"])}
        elif args.action == "distribution":
            distribution(git, gh, plan["publication"])
            values = {}
        elif args.action == "image-state":
            require(
                len(plan["publication"]["images"]) == 1,
                "image-state requires one image",
            )
            image = plan["publication"]["images"][0]
            values = {
                "version_exists": str(image_digest(image) is not None).lower(),
                "image_tag": image["tag"],
            }
        else:
            values = {
                "promote_latest": str(
                    latest(git, gh, os.environ["RELEASE_SHA"])
                ).lower(),
                "release_tag": plan["publication"]["release_tag"],
            }
            name = plan["publication"]["latest_image"]
            if name:
                values["image_tag"] = next(
                    i["tag"] for i in plan["publication"]["images"] if i["name"] == name
                )
    for name, value in values.items():
        output(name, value)


if __name__ == "__main__":
    try:
        main()
    except (PreparationError, KeyError, ValueError, OSError) as exc:
        print(f"::error::{exc}", file=sys.stderr)
        raise SystemExit(1) from exc
