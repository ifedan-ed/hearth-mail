#!/bin/bash
# ci/build-image.sh [TAG] - build the Hearth Mail server image.
#
# 1. Builds the release binary with ci/cargo.sh (pinned toolchain, --locked,
#    through loadwait, memory and CPUs capped). The release profile is one
#    fat-LTO link, so it gets more memory than a dev build (HM_MEM, default
#    24g here).
# 2. Copies the binary out of the target volume into dist/.
# 3. Lays out the image with ci/image.Dockerfile (pinned base images).
#
# The features are the ones upstream's image is built with, so the image
# reads the same stores (PostgreSQL live, RocksDB in tests).
#
# TAG defaults to hearth-mail:<short commit>. The commit is passed into the
# binary as INBUXA_REVISION, which /.well-known/source reports.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
rev="$(git rev-parse HEAD)"
tag="${1:-hearth-mail:$(git rev-parse --short=12 HEAD)}"
features="sqlite postgres mysql rocks s3 redis azure nats"

HM_MEM="${HM_MEM:-24g}" INBUXA_REVISION="$rev" ci/cargo.sh build --release --locked \
  -p inbuxa --no-default-features --features "$features"

rm -rf dist && mkdir dist
docker run --rm -v "${HM_TARGET:-hearth-mail-target}:/target:ro" -v "$root/dist:/out" \
  --user "$(id -u):$(id -g)" debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2 \
  cp /target/release/inbuxa /out/inbuxa
docker build -q --build-arg REVISION="$rev" -t "$tag" -f ci/image.Dockerfile dist
echo "built $tag ($rev)"
