# Hearth Mail build and test toolchain.
#
# One pinned image for local builds, the test run and CI, so all three compile
# with the same compiler and C toolchain. Rust is pinned by digest (a tag can
# move); clang/libclang are needed by bindgen for RocksDB and the other
# C dependencies; Node is only there so CI's actions (the build cache) can
# run in this image. Rebuild only when the pins below change, and bump the
# tag (ci/cargo.sh and .forgejo/workflows/ci.yml name it).
FROM docker.io/library/rust:1.100.0-trixie@sha256:223cc32ddca8f3ee90c436dbc94b6b5f052ea7f9653c5201e9612f08ad7d21b8
RUN export DEBIAN_FRONTEND=noninteractive && \
    apt-get update && \
    apt-get install -yq --no-install-recommends clang libclang-19-dev cmake python3 nodejs && \
    rm -rf /var/lib/apt/lists/* && \
    rustup component add rustfmt clippy
ENV CARGO_TERM_COLOR=never \
    CARGO_INCREMENTAL=0
WORKDIR /src
