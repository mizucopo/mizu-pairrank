#!/usr/bin/env bash
set -euo pipefail

latest_ref="refs/heads/automation/release-latest"
latest_tracking_ref="refs/remotes/origin/automation/release-latest"

load_latest() {
  latest_commit=""
  latest_source_sha=""
  latest_release_tag=""

  set +e
  marker_line="$(git ls-remote --exit-code --refs origin "$latest_ref")"
  marker_status=$?
  set -e
  case "$marker_status" in
    0)
      latest_commit="${marker_line%%[[:space:]]*}"
      git fetch --force origin "$latest_ref:$latest_tracking_ref"
      marker_body="$(git show -s --format=%B "$latest_tracking_ref")"
      latest_source_sha="$(printf '%s\n' "$marker_body" | sed -n 's/^source-sha: //p')"
      latest_release_tag="$(printf '%s\n' "$marker_body" | sed -n 's/^release-tag: //p')"
      if [[ ! "$latest_source_sha" =~ ^[0-9a-f]{40}$ ]] \
        || [[ ! "$latest_release_tag" =~ ^[A-Za-z0-9][A-Za-z0-9._+-]*$ ]]; then
        echo "The latest release marker is invalid." >&2
        exit 1
      fi
      ;;
    2) ;;
    *)
      echo "Could not inspect the latest release marker." >&2
      exit 1
      ;;
  esac
}

operation="${1:-read}"
case "$operation" in
  record)
    : "${RELEASE_TAG:?RELEASE_TAG is required}"
    if [[ ! "$RELEASE_TAG" =~ ^[A-Za-z0-9][A-Za-z0-9._+-]*$ ]]; then
      echo "The release tag is invalid." >&2
      exit 1
    fi

    attempt=0
    while true; do
      attempt=$((attempt + 1))
      load_latest

      if [ "$latest_source_sha" = "$GITHUB_SHA" ]; then
        if [ "$latest_release_tag" != "$RELEASE_TAG" ]; then
          echo "The current commit already has a different latest release tag." >&2
          exit 1
        fi
        exit 0
      fi

      if [ -n "$latest_source_sha" ]; then
        if git merge-base --is-ancestor "$GITHUB_SHA" "$latest_source_sha"; then
          exit 0
        fi
        if ! git merge-base --is-ancestor "$latest_source_sha" "$GITHUB_SHA"; then
          echo "The latest release marker is not on the current release history." >&2
          exit 1
        fi
      fi

      next_latest_commit="$(
        printf 'Release latest marker\n\nsource-sha: %s\nrelease-tag: %s\n' \
          "$GITHUB_SHA" "$RELEASE_TAG" \
          | git -c user.name=github-actions -c user.email=github-actions@github.com \
            commit-tree "$(git rev-parse 'HEAD^{tree}')" -p "$GITHUB_SHA"
      )"
      if git push \
        --force-with-lease="$latest_ref:$latest_commit" \
        origin "${next_latest_commit}:$latest_ref"; then
        exit 0
      fi

      expected_latest_commit="$latest_commit"
      load_latest
      if [ "$latest_commit" = "$expected_latest_commit" ]; then
        echo "Could not update the latest release marker; the remote marker did not change." >&2
        exit 1
      fi

      backoff_seconds=$((attempt < 5 ? attempt : 5))
      sleep "$backoff_seconds"
    done
    ;;
  read)
    load_latest
    if [ -z "$latest_source_sha" ]; then
      echo "The latest release marker does not exist." >&2
      exit 1
    fi
    {
      echo "source_sha=$latest_source_sha"
      echo "release_tag=$latest_release_tag"
    } >> "$GITHUB_OUTPUT"
    ;;
  *)
    echo "Usage: $0 {record|read}" >&2
    exit 2
    ;;
esac
