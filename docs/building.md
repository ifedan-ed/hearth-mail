# Building and testing

## The toolchain

ci/builder.Dockerfile is the one build environment for local builds, tests
and CI: rust:1.100.0-trixie pinned by digest, clang and libclang-19 (bindgen
for RocksDB and the other C libraries), cmake and Python (the schema
tools). It is built on first use as
`hearth-mail-builder:3`; bump the tag when it changes.

ci/cargo.sh runs cargo in it:

- through /usr/local/bin/loadwait, so it starts only when the server has
  room and runs at low priority (`NO_LOADWAIT=1` skips that, for CI);
- with the container capped: `HM_MEM` (16g), `HM_CPUS` (8), cargo jobs
  `HM_JOBS` (8);
- with core dumps off (a crashed test once left a 220 MB core file in the
  checkout, and it was committed);
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

## The end-to-end test bed

tools/hearth/e2e/ proves a release image as a running server, from the
outside, as mail clients and other mail servers meet it. Nothing in it can
reach a real domain or the host's own mail server.

- run.sh up IMAGE: the throwaway of tools/hearth/throwaway.sh (Postgres,
  the server, the console, 127.0.0.1 ports only) on a network with a fixed
  subnet (172.29.77.0/24), plus Unbound as the server's only resolver
  (test zones from local data, "no such name" for everything else) and
  sink.py, a stand-in remote mail server with STARTTLS. run.sh down
  removes all of it.
- smoke.py: the 45 basic checks (as before).
- e2e.py: outbound delivery through MX lookup with DKIM verified, bounces,
  vacation replies; inbound SPF, DKIM, DMARC and iprev, forged senders
  filed in Junk (never refused), the shipped sender-check settings; a 30 MB attachment through SMTP, IMAP and
  JMAP and a refused 108 MB one; disk quotas; search in 2,000 messages;
  JMAP WebSocket push; app passwords; autoconfig and Autodiscover.
- tools/activesync/replay.py: the ActiveSync session files the `tests`
  crate replays in process (iPhone, Android, Outlook), sent to the running
  image over HTTPS, with the WBXML code pages read from the server's own
  table.
- load.py: concurrent SMTP senders, IMAP and JMAP clients for a minute;
  every accepted message must be delivered and no request may fail.
- all.sh IMAGE runs them in order. e2e.py and load.py need dkimpy, pynacl,
  dnspython and websockets (a venv named by `PYTHON`); the rest is the
  standard library.

Results are recorded in docs/progress.md.

## CI

.forgejo/workflows/ci.yml, on the projects org's runner netcup-rootless
(rootless Docker-in-Docker, 4 CPUs and 8 GB, one job at a time, an hour per
job, no Docker socket in jobs; /srv/forgejo/runner/README.md).

- checks: formatting, the console schema and privacy catalog, the release
  helper's tests.
- build-unit: the server build and the library tests.
- integration: the integration suites that need nothing but a store on
  disk (all but automation, directory, cluster and, until its upstream
  failure is understood, imap).

The jobs run in hearth-mail-builder:3, which dind can't build. Load it after
every change of the builder (and after dind's images are wiped):

    ci/cargo.sh --version >/dev/null   # builds hearth-mail-builder:3 if missing
    docker save hearth-mail-builder:3 | docker exec -i -e DOCKER_HOST=unix:///run/user/1000/docker.sock forgejo-runner-dind docker load

There is no build cache: the jobs use no actions (fetching actions/cache
from code.forgejo.org hung and failed the first run), so each job builds
from scratch, split so each fits the runner's hour.

## The public copy on GitHub

Forgejo (projects/hearth-mail) is the main copy. github.com/ifedan-ed/hearth-mail
is a public, filtered mirror, used for builds and tests on GitHub Actions
(the maintainer, 2026-11-24). The rule: private paths stay on Forgejo; the mirror is
made only with tools/export/public-mirror.sh, never by pushing a branch
from here.

- tools/export/private-paths.txt: the paths left out of every commit (the
  cutover plan and its pre-check results, progress and decision notes, the
  builders' working rules, the Forgejo runner workflow, the export tools).
- tools/export/public-replace.txt and public-messages.txt: names, hosts,
  addresses and local paths of the live installation, replaced in every
  file and commit message; public-mailmap.txt maps the author identity.
- The script rewrites the branches (main; `BRANCHES` to
  change) on a scratch clone with git filter-repo, searches the result for
  anything forbidden, scans it with gitleaks (upstream's known test keys
  are accepted in gitleaks-upstream-fixtures.txt, each with its file and
  line), and only with `--push` force-pushes to GitHub. Run it again after
  each merge to mirror; the GitHub commits differ from Forgejo's because the
  history is rewritten.

    tools/export/public-mirror.sh           # check only
    tools/export/public-mirror.sh --push    # check, then mirror

GitHub Actions (.github/workflows/ci.yml):

- every push and pull request: formatting, the console schema, clippy, the
  server build, and the unit tests of the crates the change touches
  (ci/changed-crates.py; all of them when Cargo.toml, Cargo.lock, vendor/
  or ci/ changed);
- on demand (workflow_dispatch) and for a release tag: the whole
  workspace's unit tests and the integration suites.

Locally only the tests of the code changed run (one crate, capped jobs,
through loadwait); the full suite runs on GitHub before a release or the
cutover.

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
