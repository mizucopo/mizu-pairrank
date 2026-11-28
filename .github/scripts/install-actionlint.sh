#!/usr/bin/env bash
set -euo pipefail

# Upstream PR #654 supplies concurrency.queue support not yet in a release.
# Use the reviewed, immutable commit until an official release includes it.
revision=644076a59742c2d1540ebd4686eab3c308f0e562
output_dir="${1:-}"
if [[ -z "$output_dir" ]]; then
  output_dir="$(git rev-parse --show-toplevel)/.tools/bin"
fi

build_dir="$(mktemp -d)"
trap 'rm -rf "$build_dir"' EXIT
git init --quiet "$build_dir/source"
git -C "$build_dir/source" fetch --quiet --depth=1 \
  https://github.com/rhysd/actionlint.git "$revision"
git -C "$build_dir/source" checkout --quiet --detach "$revision"
test "$(git -C "$build_dir/source" rev-parse HEAD)" = "$revision"

(
  cd "$build_dir/source"
  GOTOOLCHAIN=local GOWORK=off go build -mod=readonly -trimpath \
    -ldflags "-X github.com/rhysd/actionlint.version=pr654-$revision" \
    -o "$build_dir/actionlint" ./cmd/actionlint
)

mkdir -p "$output_dir"
mv "$build_dir/actionlint" "$output_dir/actionlint"
"$output_dir/actionlint" -version
