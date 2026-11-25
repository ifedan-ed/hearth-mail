#!/bin/bash
# tools/hearth/e2e/run.sh up IMAGE | down
#
# The end-to-end test bed: the throwaway instance of tools/hearth/throwaway.sh
# (Postgres, the server, the console; 127.0.0.1 ports only) on a network of
# its own with a fixed subnet, plus two stand-ins for the internet:
#
# - hmproof-dns (Unbound, 172.29.77.53): the server's only resolver. It
#   answers a few test zones from local data and "no such name" for every
#   other name, so nothing the server does can reach a real domain:
#     alpha.test, beta.test  the server's own domains (MX, SPF, DMARC)
#     example.net            a well-behaved sender: SPF allows the clients'
#                            address (the network's gateway, which is where
#                            connections to the published ports come from),
#                            a DKIM key made here (selector s1), DMARC reject;
#                            its MX is the sink
#     example.org            SPF allows nobody here, DMARC reject
#     remote.example.net     MX sink.remote.example.net, for outbound mail
#   The outside domains are under IANA's reserved example.* names, not
#   .test: the spam filter checks addresses against the public suffix list,
#   where .test is missing, so a .test sender scores as an invalid address.
#   (example.com is left for the ActiveSync replays' accounts.)
#   Nothing points at the gateway's port 25: on the host that may be a live
#   mail server.
# - hmproof-sink (172.29.77.25): the MX of remote.example.net and
#   example.net, tools/hearth/e2e/sink.py, with STARTTLS (a self-signed
#   certificate); it writes what it receives to $HM_PROOF_DIR/sink.
#
# Then tools/hearth/smoke.py, tools/hearth/e2e/e2e.py,
# tools/activesync/replay.py and tools/hearth/e2e/load.py run against it.
# `down` removes all of it (throwaway.sh down).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
dir="${HM_PROOF_DIR:-/srv/hearth-mail-proof}"
subnet=172.29.77.0/24
gateway=172.29.77.1
server_ip=172.29.77.10
dns_ip=172.29.77.53
sink_ip=172.29.77.25
unbound_image='klutchell/unbound:v1.26.2@sha256:de8e624f84337bdb2d16540c0b203d5df2a981180421cb50ab4212b67542d8b4'
python_image='python:3.13-slim'

case "${1:-}" in
up)
  image="${2:?image}"
  mkdir -p "$dir/keys" "$dir/sink"
  chmod 700 "$dir"
  # The test sender's DKIM key and the sink's certificate, made fresh.
  openssl genrsa -out "$dir/keys/outside.key" 2048 2>/dev/null
  pub=$(openssl rsa -in "$dir/keys/outside.key" -pubout -outform DER 2>/dev/null | base64 -w0)
  openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj /CN=sink.remote.example.net \
    -keyout "$dir/keys/sink.key" -out "$dir/keys/sink.crt" 2>/dev/null
  # A TXT string holds at most 255 bytes: split the key.
  dkim_txt="\"v=DKIM1; k=rsa; p=\""
  for ((i = 0; i < ${#pub}; i += 200)); do dkim_txt+=" \"${pub:i:200}\""; done
  cat > "$dir/unbound.conf" <<EOF
server:
    do-daemonize: no
    username: ""
    chroot: ""
    pidfile: ""
    logfile: ""
    use-syslog: no
    directory: "/tmp"
    verbosity: 1
    interface: 0.0.0.0
    port: 53
    access-control: 0.0.0.0/0 refuse
    access-control: $subnet allow
    access-control: 127.0.0.0/8 allow
    do-ip6: no
    module-config: "iterator"
    # Nothing but the test zones: every other name is answered "no such
    # name" at once, as the root would for a name that doesn't exist
    # (REFUSED would reach the server as SERVFAIL through Docker's resolver,
    # and DMARC's lookups would end as temperror).
    local-zone: "." static
    local-zone: "alpha.test." static
    local-zone: "beta.test." static
    local-zone: "example.net." static
    local-zone: "example.org." static
    local-zone: "77.29.172.in-addr.arpa." static
    local-data: "alpha.test. 60 IN MX 10 mail.alpha.test."
    local-data: "mail.alpha.test. 60 IN A $server_ip"
    local-data: 'alpha.test. 60 IN TXT "v=spf1 a:mail.alpha.test -all"'
    local-data: '_dmarc.alpha.test. 60 IN TXT "v=DMARC1; p=reject"'
    local-data: "beta.test. 60 IN MX 10 mail.alpha.test."
    local-data: 'beta.test. 60 IN TXT "v=spf1 a:mail.alpha.test -all"'
    local-data: '_dmarc.beta.test. 60 IN TXT "v=DMARC1; p=reject"'
    local-data-ptr: "$server_ip mail.alpha.test."
    # Its MX is the sink too (replies to it, vacation answers): never the
    # gateway's address, whose port 25 on the host may be a live server.
    local-data: "example.net. 60 IN MX 10 sink.remote.example.net."
    local-data: "client.example.net. 60 IN A $gateway"
    local-data-ptr: "$gateway client.example.net."
    local-data: 'example.net. 60 IN TXT "v=spf1 ip4:$gateway -all"'
    local-data: '_dmarc.example.net. 60 IN TXT "v=DMARC1; p=reject"'
    local-data: 's1._domainkey.example.net. 60 IN TXT $dkim_txt'
    local-data: "example.org. 60 IN MX 10 mail.example.org."
    local-data: "mail.example.org. 60 IN A 192.0.2.1"
    local-data: 'example.org. 60 IN TXT "v=spf1 ip4:192.0.2.1 -all"'
    local-data: '_dmarc.example.org. 60 IN TXT "v=DMARC1; p=reject"'
    local-data: "remote.example.net. 60 IN MX 10 sink.remote.example.net."
    local-data: "sink.remote.example.net. 60 IN A $sink_ip"
    local-data-ptr: "$sink_ip sink.remote.example.net."
EOF
  chmod 644 "$dir/unbound.conf" "$dir/keys/sink.crt" "$dir/keys/sink.key"
  chmod 777 "$dir/sink"
  docker network inspect hmproof >/dev/null 2>&1 || docker network create --subnet "$subnet" hmproof >/dev/null
  docker run -d --name hmproof-dns --network hmproof --ip "$dns_ip" --user 101:102 \
    --sysctl net.ipv4.ip_unprivileged_port_start=53 --read-only --tmpfs /tmp \
    --cap-drop ALL --memory 64m -v "$dir/unbound.conf:/etc/unbound/unbound.conf:ro" \
    "$unbound_image" >/dev/null
  docker run -d --name hmproof-sink --network hmproof --ip "$sink_ip" --memory 128m \
    -v "$here/sink.py:/sink.py:ro" -v "$dir/keys:/certs:ro" -v "$dir/sink:/out" \
    "$python_image" python3 -u /sink.py >/dev/null
  HM_PROOF_SUBNET="$subnet" \
  HM_PROOF_SERVER_ARGS="--ip $server_ip --dns $dns_ip -e INBUXA_HOSTNAME=mail.alpha.test ${HM_E2E_SERVER_ARGS:-}" \
    "$here/../throwaway.sh" up "$image"
  ;;
down)
  "$here/../throwaway.sh" down
  ;;
*)
  echo "usage: $0 up IMAGE | down" >&2; exit 2 ;;
esac
