#!/usr/bin/env python3
"""Number main once, then build and publish that exact commit in the same run."""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from urllib import error, parse, request

POLICY = ".github/release.json"
LEVELS = {"patch", "minor", "major"}
SHA = re.compile(r"[0-9a-f]{40}\Z")
SEMVER = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-((?:0|[1-9][0-9]*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9][0-9]*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*))*))?"
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
            {**image, "tag": image["tag"].format_map(context)}
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
        if (
            scheme == "semver"
            and SEMVER.fullmatch(base_version).group(4)
            and level != "patch"
        ):
            require(
                explicit_minimum is not None,
                "Prerelease core/stable transition needs an explicit version minimum",
            )
        version = bump(base_version, level, scheme)
        if explicit_minimum is not None:
            require(
                version_key(explicit_minimum, scheme) >= version_key(version, scheme),
                f"Explicit minimum {explicit_minimum} does not meet {level} "
                f"increment from {base_version}",
            )
            version = explicit_minimum
        revision = None
    for _ in range(100):
        plan = publication(policy, version, revision)
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


def read_version(git, tree, policy):
    specs = policy["version"]["sources"]
    require(specs, "No version sources declared")
    values = [read_field(git.blob(tree, spec["path"]), spec) for spec in specs]
    require(len(set(values)) == 1, "Declared version sources disagree")
    scheme = policy["version"]["scheme"]
    version_key(values[0], "semver" if scheme == "upstream-revision" else scheme)
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
        url = self.root + path
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
    require(
        len(labels) == 1 and labels[0][8:] in LEVELS,
        f"PR #{pr['number']} needs exactly one release:patch/minor/major label",
    )
    require(
        bool((pr.get("body") or "").strip()),
        f"PR #{pr['number']} needs a classification reason in its body",
    )
    return labels[0][8:]


def record_at(git, commit):
    message = git.text("show", "-s", "--format=%B", commit)
    if "Repo-Template-Release: 1" not in message.splitlines():
        return None
    fields = {}
    for line in message.splitlines():
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
    return [record_at(git, commit) for commit in commits.splitlines()]


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


def prepare(git, gh, run_id, control_commit=None):
    require(str(run_id).isdigit(), "GITHUB_RUN_ID is required")
    require(
        gh.repo()["default_branch"] == "main", "Release requires default branch main"
    )
    for _ in range(3):
        main = git.fetch_main()
        history = records(git, main)
        prior = next((r for r in history if r["Run"] == str(run_id)), None)
        if prior:
            return prepared(git, prior)
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
                return {"publish": "false"}
        policy = policy_at(git, main)
        last = history[0] if history else None
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
            parents = git.text(
                "rev-list", "--parents", "-n", "1", introductions[-1]
            ).split()
            boundary = parents[1] if len(parents) == 2 else None
        commits = git.ancestors(main)
        pending = list(
            reversed(commits[: commits.index(boundary)] if boundary else commits)
        )
        if not pending:
            return {"publish": "false"}
        levels, numbers = [], []
        for commit in pending:
            require(
                len(git.text("rev-list", "--parents", "-n", "1", commit).split()) <= 2,
                "Use squash merge only",
            )
            matches = [
                p
                for p in gh.pages(f"/commits/{commit}/pulls")
                if p.get("merged_at")
                and p["base"]["ref"] == "main"
                and p.get("merge_commit_sha") == commit
            ]
            if boundary is None and commit == commits[-1] and not matches:
                # A new repository starts with its configured version.
                continue
            require(
                len(matches) == 1, f"Commit {commit} needs exactly one squash-merged PR"
            )
            pr = gh.repo(f"/pulls/{matches[0]['number']}")
            levels.append(classification(pr))
            numbers.append(pr["number"])
        version = read_version(git, main, policy)
        revision_path = policy["version"].get("revision_path")
        revision = (
            (git.blob(main, revision_path, True) or b"r0").decode().strip()
            if revision_path
            else None
        )
        level = (
            max(levels, key=lambda v: ["patch", "minor", "major"].index(v))
            if levels
            else "patch"
        )
        baseline = last["commit"] if last else boundary
        if baseline and not last:
            entries = git.entries(baseline)
            if any(
                spec["path"] not in entries for spec in policy["version"]["sources"]
            ):
                baseline = None
        base_version = read_version(git, baseline, policy) if baseline else version
        base_revision = (
            (git.blob(baseline, revision_path, True) or b"r0").decode().strip()
            if baseline and revision_path
            else revision
        )
        explicit = version if version != base_version and not revision_path else None
        if not levels:
            # choose() bumps its base; reserve the initial candidate directly instead.
            candidate = publication(policy, version, revision)
            if not remote_collisions(gh, candidate):
                plan = candidate
            else:
                version, revision, plan = choose(
                    policy,
                    base_version,
                    level,
                    None,
                    lambda p: remote_collisions(gh, p),
                    base_revision=base_revision,
                    input_version=version,
                )
        else:
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


def latest(git, gh, commit):
    completed = [
        r for r in gh.pages("/releases") if not r["draft"] and not r.get("prerelease")
    ]
    require(
        len({r["tag_name"] for r in completed}) == len(completed),
        "Ambiguous completed releases",
    )
    releases = {r["tag_name"]: r for r in completed}
    for record in records(git, git.fetch_main()):
        if record["Tag"] in releases:
            prepared(git, record)
            return record["commit"] == commit
    raise PreparationError("No completed release exists on main")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "action", choices=["prepare", "plan", "latest", "image-state", "classification"]
    )
    parser.add_argument("--root", default=".")
    args = parser.parse_args()
    git, gh = Git(args.root), GitHub()
    if args.action == "classification":
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
        classification(gh.repo(f"/pulls/{event['number']}"))
        return
    require(os.environ.get("GITHUB_REF") == "refs/heads/main", "Release requires main")
    if args.action == "prepare":
        values = prepare(git, gh, os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_SHA"])
    else:
        plan = checkout_plan(git)
        if args.action == "plan":
            Path(os.environ["RELEASE_PLAN_OUTPUT"]).write_bytes(canonical(plan))
            values = {"version": plan["generated_version"]}
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
