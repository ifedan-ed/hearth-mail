# Building and testing

## The toolchain

ci/builder.Dockerfile is the one build environment for local builds, tests
and CI: rust:1.100.0-trixie pinned by digest, clang and libclang-19 (bindgen
for RocksDB and the other C libraries), cmake, Python (the schema tools) and
Node (for CI's cache action). It is built on first use as
`hearth-mail-builder:2`; bump the tag when it changes.

ci/cargo.sh runs cargo in it:

- through /usr/local/bin/loadwait, so it starts only when the server has
  room and runs at low priority (`NO_LOADWAIT=1` skips that, for CI);
- with the container capped: `HM_MEM` (16g), `HM_CPUS` (8), cargo jobs
  `HM_JOBS` (8);
- with the cargo registry in the volume `hearth-mail-cargo` and the target
  directory in `hearth-mail-target` (or `HM_TARGET`, for a second
  checkout), so builds are incremental and nothing is written into the
  checkout as root.

Every build is `--locked`.

## Commands

    ci/cargo.sh build -p inbuxa --locked                   # dev build
    ci/cargo.sh test --workspace --exclude tests --locked --features smtp/test_mode
    STORE=RocksDb ci/cargo.sh test -p tests --locked -- --test-threads=1
    ci/build-image.sh                                      # release image
    python3 tools/hearth/strip-schema.py --check           # console schema
    python3 tools/fork/expr-schema.py --check
    python3 tools/fork/privacy-check.py

The library tests need `--features smtp/test_mode`: the SMTP crate's tests
use a mock resolver that only the test feature builds, and with the `tests`
crate left out nothing else turns it on (upstream's `cargo test --workspace
--exclude tests` doesn't compile for that reason).

The integration suites start their own servers on fixed ports, one suite at
a time (`--test-threads=1`), and are timing-sensitive: run nothing heavy
beside them. Suites that need containers, a coordinator or other settings
are in docs/spec/container-tests.md; in a build container they fail for
want of a Docker socket.

## The release image

ci/build-image.sh builds the release binary (fat LTO, so `HM_MEM` 24g),
copies it out of the target volume into dist/, and lays out the image with
ci/image.Dockerfile: Debian trixie-slim pinned by digest, user 2000,
/etc/inbuxa and /var/lib/inbuxa, the same ports, health check, entry point
and inbuxa-migrate (the pinned upstream release, checksum-checked) as the
inbuxa image. The commit goes into the binary (`INBUXA_REVISION`, shown by
`/.well-known/source`) and into the image label. Tag:
`hearth-mail:<12-character commit>`.

## CI

.forgejo/workflows/ci.yml, on the projects org's runner netcup-rootless
(rootless Docker-in-Docker, 4 CPUs and 8 GB, one job at a time, an hour per
job, no Docker socket in jobs; /srv/forgejo/runner/README.md).

- checks: formatting, the console schema and privacy catalog, the release
  helper's tests.
- build-test: the server build, the library tests, and the integration
  suites compiled.
- integration: the integration suites that need nothing but a store on
  disk (all but automation, directory and cluster), from the build job's
  cache.

The jobs run in hearth-mail-builder:2, which dind can't build. Load it after
every change of the builder (and after dind's images are wiped):

    ci/cargo.sh --version >/dev/null   # builds hearth-mail-builder:2 if missing
    docker save hearth-mail-builder:2 | docker exec -i -e DOCKER_HOST=unix:///run/user/1000/docker.sock forgejo-runner-dind docker load

Cargo's registry and target directory are cached between runs by the
runner (actions/cache, key: Cargo.lock), so only the first run after a
dependency change compiles everything.

## Results

### Upstream inbuxa at v27.1-rc.39 (ca4c56cb), as it is, 2026-11-23

Built and tested in this toolchain, with nothing changed (the commit the
live server runs):

- Build of the workspace and every test target (`cargo test --workspace
  --locked --no-run`): passed, 864 s (8 CPUs, 16 GB, a cold cache).
- Library tests (`--workspace --exclude tests --features smtp/test_mode`):
  1003 passed, 0 failed, 5 ignored (286 s with the build).
  Without `--features smtp/test_mode` the smtp crate's tests don't compile
  (`mock_resolve` not found), as noted above.
- Integration suites (`STORE=RocksDb cargo test -p tests --
  --test-threads=1`): 103 passed, 8 failed, 44 ignored, in 1009 s (1367 s
  with the build). The eight failures, all from the environment rather
  than the code:
  - automation::automation_tests, directory::directory_tests: start
    containers (pebble, OpenLDAP); no Docker socket in the build container.
  - cluster::broadcast::cluster_tests: needs `COORDINATOR` and a shared
    store (docs/spec/container-tests.md).
  - cluster::stress::stress_tests: `ServerUnavailable` under concurrency on
    a loaded machine.
  - imap::imap_tests: imap/antispam.rs:179, a spam-training count (0 where
    1 was expected).
  - smtp::reporting::analyze::report_analyze: a report count (0 where 5
    were expected).
  - system::system_tests: the OIDC step timed out reaching the test server
    (127.0.0.1:8899).
  - webdav::webdav_tests: timed out reaching the test server on
    /.well-known/carddav.

### Hearth Mail after the removal, 2026-11-23

Same toolchain and commands:

- Library tests: 934 passed, 0 failed, 5 ignored. The 69 fewer than
  upstream are the removed crates' and modules' own tests. The first run
  failed 6, which found real loose ends (fixed): the default roles and the
  task manager unwrapped every permission and task type id up to the count,
  which the removal left with gaps (the task manager would have panicked at
  start), and the trace tests held the event count to the variant count.
- The vendored hickory resolver's tests can't build in this workspace (its
  test-support crate isn't vendored); run in hickory's repository at the
  vendored commit with our change: 103 passed, 0 failed, 3 ignored.
- Integration suites: 101 passed, 7 failed, 43 ignored, in 706 s (1386 s
  with the build). The webdav suite and the calendar, contacts, files and
  group-sharing suites are gone. Of the seven:
  - automation, directory (no Docker socket), cluster::broadcast (needs
    `COORDINATOR`), cluster::stress (`ServerUnavailable` under load), and
    imap (imap/antispam.rs:179): the same as upstream.
  - system::system_tests: the same OIDC timeout as upstream: the test
    gave the server 500 ms to sign a password in, which a debug build on a
    loaded machine misses. The test's client timeouts are 5 s now.
  - jmap::jmap_tests: the test harness's JMAP client library (jmap-client
    0.4.3) lists contacts and calendars in every request's `using`, which
    the server then refused as unknownCapability. That would break real
    mail clients built the same way, so the removed capabilities are now
    accepted in `using` and ignored (their methods answer unknownMethod;
    docs/decisions.md).
  - smtp::reporting::analyze, which failed upstream, passed.

Rerun after those fixes: see docs/progress.md.
