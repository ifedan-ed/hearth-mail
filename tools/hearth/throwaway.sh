#!/bin/bash
# tools/hearth/throwaway.sh up IMAGE | down
#
# A throwaway Hearth Mail instance for proving an image, never the live
# server: its own Docker network, a fresh Postgres (the live server's
# postgres:16-alpine digest), the server, and the admin console, on
# 127.0.0.1 ports only. State lives in $HM_PROOF_DIR (default
# /srv/hearth-mail-proof) and goes with `down`.
#
# `up` starts the server in bootstrap mode with a recovery administrator,
# then tools/hearth/smoke.py does the setup (bootstrap, domains, users) and
# the checks. Ports (127.0.0.1): SMTP 22525, submissions 22465, submission
# 22587, IMAP 22143, IMAPS 22993, POP3S 22995, ManageSieve 24190, HTTP
# 28080, HTTPS 28443, console 28081.
set -euo pipefail
dir="${HM_PROOF_DIR:-/srv/hearth-mail-proof}"
net=hmproof
pg_image='postgres:16-alpine@sha256:0cefaabd882e42bcb2a2f3dc56d3ce4770560ba62fd7f197f1a2e008fd5731ab'
admin_image='registry.coffeylabs.org/inbuxa/inbuxa-admin:27.1-rc.24@sha256:7c03ef137f5f0d599d5735f72e5c28ae90851f7906afd206df240a3a86b95afe'

case "${1:-}" in
up)
  image="${2:?image}"
  mkdir -p "$dir/etc" "$dir/data" "$dir/pg"
  chmod 700 "$dir"
  # Throwaway secrets, made here and kept only in the proof directory.
  [ -s "$dir/pg-password" ] || (umask 077; head -c 24 /dev/urandom | base64 | tr -d '/+=' > "$dir/pg-password")
  [ -s "$dir/recovery-admin" ] || (umask 077; printf 'admin:%s' "$(head -c 18 /dev/urandom | base64 | tr -d '/+=')" > "$dir/recovery-admin")
  chown -R 2000:2000 "$dir/etc" "$dir/data" "$dir/recovery-admin"
  docker network inspect "$net" >/dev/null 2>&1 || docker network create "$net" >/dev/null
  docker run -d --name hmproof-pg --network "$net" --memory 1g \
    -e POSTGRES_DB=inbuxa -e POSTGRES_USER=inbuxa -e POSTGRES_PASSWORD_FILE=/run/pg-password \
    -v "$dir/pg-password:/run/pg-password:ro" -v "$dir/pg:/var/lib/postgresql/data" \
    "$pg_image" >/dev/null
  until docker exec hmproof-pg pg_isready -U inbuxa -d inbuxa >/dev/null 2>&1; do sleep 1; done
  docker run -d --name hmproof-server --network "$net" --hostname hmproof-server --init \
    --read-only --tmpfs /tmp:size=256m,uid=2000,gid=2000 --cap-drop ALL \
    --security-opt no-new-privileges:true --memory 2g --pids-limit 512 \
    -e INBUXA_RECOVERY_ADMIN_FILE=/run/recovery-admin \
    -e INBUXA_HTTP_BASIC_AUTH=all \
    -e INBUXA_PUBLIC_URL=http://127.0.0.1:28080 \
    -e INBUXA_ADMIN_URL=http://127.0.0.1:28081 \
    -v "$dir/recovery-admin:/run/recovery-admin:ro" \
    -v "$dir/etc:/etc/inbuxa" -v "$dir/data:/var/lib/inbuxa" \
    -p 127.0.0.1:22525:25 -p 127.0.0.1:22465:465 -p 127.0.0.1:22587:587 \
    -p 127.0.0.1:22143:143 -p 127.0.0.1:22993:993 -p 127.0.0.1:22995:995 \
    -p 127.0.0.1:24190:4190 -p 127.0.0.1:28080:8080 -p 127.0.0.1:28443:443 \
    "$image" >/dev/null
  docker run -d --name hmproof-admin --network "$net" --cap-drop ALL --memory 128m \
    -e API_BASE_URL=http://127.0.0.1:28080 -p 127.0.0.1:28081:8080 "$admin_image" >/dev/null
  echo "up: $image (state in $dir)"
  ;;
down)
  docker rm -f hmproof-admin hmproof-server hmproof-pg >/dev/null 2>&1 || true
  docker network rm "$net" >/dev/null 2>&1 || true
  rm -rf "$dir"
  echo "down: containers, network and $dir removed"
  ;;
*)
  echo "usage: $0 up IMAGE | down" >&2; exit 2 ;;
esac
