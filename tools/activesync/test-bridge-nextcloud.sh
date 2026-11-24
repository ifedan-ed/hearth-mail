#!/bin/bash
# tools/activesync/test-bridge-nextcloud.sh - the ActiveSync replays with the
# DAV bridge on, against a throwaway Nextcloud
# (tests/resources/activesync/nextcloud.txt, run by the tests crate's
# activesync suite).
#
# What it does:
#   1. starts the official Nextcloud image on SQLite in its own Docker
#      network, with a generated admin password and a memory cap;
#   2. makes the test accounts there (ann@example.com and bob@example.com,
#      with the passwords the suite gives them), so the bridge can sign in
#      with the devices' own credentials (INBUXA_DAV_AUTH=passthrough);
#   3. builds the tests crate with ci/cargo.sh and runs the suite's binary
#      in the toolchain image on the Nextcloud's network;
#   4. removes the container, network, volume and temporary files, whatever
#      happened (a trap), and never touches images.
#
# Never pointed at a real Nextcloud.
#
# Example:
#   HM_JOBS=8 HM_CPUS=8 HM_MEM=16g tools/activesync/test-bridge-nextcloud.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
image="${NEXTCLOUD_IMAGE:-nextcloud:35.0.3-apache}"
builder="${HM_IMAGE:-hearth-mail-builder:3}"
target="${HM_TARGET:-hearth-mail-target}"
name="hm-bridge-test-$$"
network="$name"
volume="$name-data"
tmp="$(mktemp -d)"

cleanup() {
  docker rm -f "$name" >/dev/null 2>&1 || true
  docker rm -f "$name-run" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
  docker volume rm "$volume" >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT

docker image inspect "$image" >/dev/null 2>&1 || docker pull -q "$image" >/dev/null

# The throwaway admin password, kept in a private env file rather than on
# a command line.
password="$(head -c 24 /dev/urandom | base64 | tr -dc 'A-Za-z0-9')"
umask 077
cat >"$tmp/nextcloud.env" <<EOF
SQLITE_DATABASE=nextcloud
NEXTCLOUD_ADMIN_USER=admin
NEXTCLOUD_ADMIN_PASSWORD=$password
NEXTCLOUD_TRUSTED_DOMAINS=nextcloud 127.0.0.1 localhost
EOF
cat >"$tmp/test.env" <<EOF
INBUXA_DAV_URL=http://nextcloud/remote.php/dav
INBUXA_DAV_AUTH=passthrough
STORE=RocksDb
RUST_MIN_STACK=8388608
EOF

docker network create "$network" >/dev/null
docker volume create "$volume" >/dev/null
echo "Starting $image as $name"
/usr/local/bin/loadwait docker run -d --name "$name" \
  --network "$network" --network-alias nextcloud \
  --memory 2g --memory-swap 2g --cpus 2 \
  -p 127.0.0.1::80 \
  --env-file "$tmp/nextcloud.env" \
  -v "$volume:/var/www/html" \
  "$image" >/dev/null
port="$(docker port "$name" 80/tcp | head -n1 | sed 's/.*://')"
echo "Nextcloud on http://127.0.0.1:$port, installing"

# Installation (copying the code, creating the SQLite database) takes a
# minute or two.
for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:$port/status.php" 2>/dev/null | grep -q '"installed":true'; then
    break
  fi
  if ! docker ps -q --filter "name=^$name$" | grep -q .; then
    docker logs --tail 50 "$name" >&2
    echo "Nextcloud stopped during installation" >&2
    exit 1
  fi
  sleep 5
done
status="$(curl -fsS "http://127.0.0.1:$port/status.php")"
echo "$status" | grep -q '"installed":true' || {
  echo "Nextcloud did not finish installing" >&2
  exit 1
}
echo "Installed: $status"

# The suite's accounts, with the suite's passwords
# (tests/src/activesync/mod.rs).
for account in "ann@example.com:ann secret + extra safety" "bob@example.com:bob secret + extra safety"; do
  user="${account%%:*}"
  docker exec -u www-data -e OC_PASS="${account#*:}" "$name" \
    php occ user:add --password-from-env "$user" >/dev/null
done

# Build the suite and find its binary (a path inside the target volume).
STORE=RocksDb "$root/ci/cargo.sh" test -p tests --no-run --message-format=json \
  >"$tmp/build.json"
binary="$(python3 -I -c '
import json, sys
for line in open(sys.argv[1]):
    if not line.startswith("{"):
        continue
    message = json.loads(line)
    if message.get("reason") == "compiler-artifact" and message.get("executable") \
            and message["target"]["name"] == "tests":
        print(message["executable"])
' "$tmp/build.json" | tail -n1)"
[ -n "$binary" ] || {
  echo "could not find the test binary" >&2
  exit 1
}

echo "Running $binary"
/usr/local/bin/loadwait docker run --rm --init --name "$name-run" \
  --network "$network" \
  --memory 4g --cpus 4 \
  --env-file "$tmp/test.env" \
  -v "$target:/target" -v "$root:/src" -w /src/tests \
  "$builder" "$binary" activesync::activesync_tests --nocapture --test-threads=1
