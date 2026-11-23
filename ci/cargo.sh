#!/bin/bash
# ci/cargo.sh ARGS... - run cargo in the pinned build image (ci/builder.Dockerfile).
#
# Why: a full build of this workspace compiles RocksDB, aws-lc and ~900 crates
# and its release link is a single fat-LTO step, so an uncapped build can take
# most of the machine. This wrapper:
#   - waits for room and runs at low priority through /usr/local/bin/loadwait
#     (skipped when NO_LOADWAIT=1, as in CI where the runner is the limit);
#   - caps the container's memory (HM_MEM, default 16g) and CPUs (HM_CPUS,
#     default 8) and cargo's parallel jobs (HM_JOBS, default 8);
#   - keeps the cargo registry and the target directory in named volumes
#     (hearth-mail-cargo, and hearth-mail-target or HM_TARGET), so repeat builds are incremental
#     and nothing is written into the checkout as root.
#
# Examples:
#   ci/cargo.sh build -p inbuxa --locked
#   HM_TARGET=hearth-mail-target-wt ci/cargo.sh check   (a second checkout)
#   ci/cargo.sh test --workspace --exclude tests --locked
#   HM_MEM=24g ci/cargo.sh build -p inbuxa --release --locked
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
image="${HM_IMAGE:-hearth-mail-builder:3}"
docker image inspect "$image" >/dev/null 2>&1 ||
  docker build -q -t "$image" -f "$root/ci/builder.Dockerfile" "$root/ci" >/dev/null

run=(docker run --rm --init
  --name "hearth-mail-cargo-$$"
  --memory "${HM_MEM:-16g}" --memory-swap "${HM_MEM:-16g}"
  --cpus "${HM_CPUS:-8}"
  -e CARGO_BUILD_JOBS="${HM_JOBS:-8}"
  -e CARGO_TARGET_DIR=/target
  -e RUST_MIN_STACK=8388608
  -e TMPDIR=/target/tmp
  -v hearth-mail-cargo:/usr/local/cargo/registry
  -v "${HM_TARGET:-hearth-mail-target}:/target"
  -v "$root:/src"
  -w /src)
# Pass the test harness's settings through when they are set.
for v in STORE COORDINATOR LOG SPAM_RULES_URL RUST_LOG RUST_BACKTRACE INBUXA_REVISION; do
  [ -n "${!v:-}" ] && run+=(-e "$v=${!v}")
done
run+=("$image" bash -c 'mkdir -p /target/tmp && exec cargo "$@"' cargo "$@")

if [ "${NO_LOADWAIT:-}" = 1 ]; then exec "${run[@]}"; fi
exec /usr/local/bin/loadwait "${run[@]}"
