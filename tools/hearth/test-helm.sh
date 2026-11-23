#!/usr/bin/env bash
# Test the Helm chart (deploy/helm/hearth-mail) on a throwaway kind cluster,
# then remove the cluster and the downloaded tools:
#
#   HM_IMAGE=hearth-mail:<commit> tools/hearth/test-helm.sh
#
# It proves: the chart installs and the server becomes ready on PostgreSQL;
# the Unbound sidecar is the pod's resolver and validates DNSSEC (a signed
# name gets the AD flag, a deliberately broken one is refused); the server
# logs no "cannot validate DNSSEC"; in-cluster names still resolve (the
# database's); the console's delivery test to posteo.de finds TLSA records
# and verifies DANE; the recovery administrator signs in. Needs the
# internet. kind, kubectl and helm are pinned and checksum-checked; heavy
# steps go through /usr/local/bin/loadwait.
set -euo pipefail
cd "$(dirname "$0")/../.."

KIND_VERSION=v0.33.0
KUBECTL_VERSION=v1.37.2
HELM_VERSION=v4.3.2
CLUSTER=hm-helm-test-$$
IMAGE=${HM_IMAGE:?set HM_IMAGE to a built hearth-mail image}
NS=hearth-mail
LOADWAIT=/usr/local/bin/loadwait
UNBOUND=klutchell/unbound:v1.26.2@sha256:de8e624f84337bdb2d16540c0b203d5df2a981180421cb50ab4212b67542d8b4
BUSYBOX=busybox:1.37@sha256:9db7b59979c38555a39def84a31fb98b5296952f9e3afd4f6f11f05b07adfab0
POSTGRES=postgres:16-alpine@sha256:0cefaabd882e42bcb2a2f3dc56d3ce4770560ba62fd7f197f1a2e008fd5731ab
ADMIN=registry.coffeylabs.org/inbuxa/inbuxa-admin:27.1-rc.24@sha256:7c03ef137f5f0d599d5735f72e5c28ae90851f7906afd206df240a3a86b95afe

work=$(mktemp -d)
pf_pid=
fails=0
cleanup() {
  status=$?
  if [ "$status" != 0 ] || [ "$fails" != 0 ]; then
    if [ -s "${KUBECONFIG:-/nonexistent}" ]; then
      "$work/kubectl" get pods -A 2>/dev/null | grep -v kube-system || true
      for p in $("$work/kubectl" -n "$NS" get pods -o name 2>/dev/null); do
        echo "-- $p"; "$work/kubectl" -n "$NS" logs "$p" --tail=30 --all-containers 2>/dev/null || true
      done
    fi
  fi
  [ -n "$pf_pid" ] && kill "$pf_pid" 2>/dev/null || true
  [ -x "$work/kind" ] && "$work/kind" delete cluster --name "$CLUSTER" >/dev/null 2>&1 || true
  # kind leaves its Docker network behind; remove it when nothing uses it.
  if [ "$(docker network inspect kind --format '{{len .Containers}}' 2>/dev/null)" = 0 ]; then
    docker network rm kind >/dev/null 2>&1 || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT
pass() { echo "PASS  $*"; }
fail() { echo "FAIL  $*"; fails=$((fails + 1)); }
check() { if eval "$1"; then pass "$2"; else fail "$2"; fi; }

# ------------------------------------------------------------- tools ----
(
  cd "$work"
  curl -fsSLo kind "https://github.com/kubernetes-sigs/kind/releases/download/$KIND_VERSION/kind-linux-amd64"
  curl -fsSL "https://github.com/kubernetes-sigs/kind/releases/download/$KIND_VERSION/kind-linux-amd64.sha256sum" | awk '{print $1"  kind"}' | sha256sum -c --quiet -
  curl -fsSLo kubectl "https://dl.k8s.io/release/$KUBECTL_VERSION/bin/linux/amd64/kubectl"
  echo "$(curl -fsSL "https://dl.k8s.io/release/$KUBECTL_VERSION/bin/linux/amd64/kubectl.sha256")  kubectl" | sha256sum -c --quiet -
  curl -fsSLo helm.tgz "https://get.helm.sh/helm-$HELM_VERSION-linux-amd64.tar.gz"
  echo "$(curl -fsSL "https://get.helm.sh/helm-$HELM_VERSION-linux-amd64.tar.gz.sha256sum" | awk '{print $1}')  helm.tgz" | sha256sum -c --quiet -
  tar xzf helm.tgz --strip-components=1 linux-amd64/helm && rm helm.tgz
  chmod +x kind kubectl helm
)
export KUBECONFIG="$work/kubeconfig"
K="$work/kubectl"
H="$work/helm"
pass "tools: kind $KIND_VERSION, kubectl $KUBECTL_VERSION, helm $HELM_VERSION (checksums verified)"

# ------------------------------------------------- lint and template ----
"$H" lint deploy/helm/hearth-mail --strict \
  --set image.tag=t,hostname=mail.example.org,publicUrl=https://mail.example.org,postgres.host=pg,postgres.passwordSecret.name=pg,unbound.clusterDnsIp=10.96.0.10 >/dev/null
pass "helm lint --strict"
if "$H" template x deploy/helm/hearth-mail --set unbound.clusterDnsIp=10.96.0.10 >/dev/null 2>&1; then
  fail "template without the required values is refused"
else
  pass "template without the required values is refused"
fi

# ---------------------------------------------------- cluster, images ----
t0=$(date +%s)
$LOADWAIT "$work/kind" create cluster --name "$CLUSTER" --wait 120s >/dev/null 2>&1
pass "cluster $CLUSTER up in $(( $(date +%s) - t0 ))s"
# Pulled images are multi-platform indexes holding one platform's layers,
# which kind's import refuses; a one-line local build makes each a plain
# single-platform image under the name the chart uses.
for img in "$IMAGE" "$UNBOUND" "$BUSYBOX" "$POSTGRES" "$ADMIN"; do
  name=${img%@*}
  had_tag=0; docker image inspect "$name" >/dev/null 2>&1 && had_tag=1
  printf 'FROM %s\n' "$img" | $LOADWAIT docker build -q -t "hm-helm-test/$name" - >/dev/null
  docker tag "hm-helm-test/$name" "$name"
  "$work/kind" load docker-image --name "$CLUSTER" "$name" >/dev/null
  docker rmi "hm-helm-test/$name" >/dev/null
  # Drop only a tag this made; a tag that was here stays.
  [ "$had_tag" = 1 ] || docker rmi "$name" >/dev/null 2>&1 || true
done
pass "images loaded"

# ------------------------------------------------------------ database ----
dbpass=$(head -c 18 /dev/urandom | base64 | tr -d '/+=')
admin_pass=$(head -c 18 /dev/urandom | base64 | tr -d '/+=')
$K create namespace db >/dev/null
$K -n db create secret generic pg --from-literal=password="$dbpass" >/dev/null
$K -n db apply -f - >/dev/null <<EOF
apiVersion: apps/v1
kind: Deployment
metadata: {name: postgres}
spec:
  selector: {matchLabels: {app: postgres}}
  template:
    metadata: {labels: {app: postgres}}
    spec:
      containers:
        - name: postgres
          image: ${POSTGRES%@*}
          imagePullPolicy: Never
          env:
            - {name: POSTGRES_DB, value: inbuxa}
            - {name: POSTGRES_USER, value: inbuxa}
            - {name: POSTGRES_PASSWORD, valueFrom: {secretKeyRef: {name: pg, key: password}}}
          readinessProbe: {exec: {command: [pg_isready, -U, inbuxa]}, periodSeconds: 3}
---
apiVersion: v1
kind: Service
metadata: {name: postgres}
spec:
  selector: {app: postgres}
  ports: [{port: 5432}]
EOF
$K -n db wait --for=condition=available deploy/postgres --timeout=180s >/dev/null
pass "PostgreSQL ready"

# ------------------------------------------------------------- install ----
$K create namespace "$NS" >/dev/null
$K -n "$NS" create secret generic pg --from-literal=password="$dbpass" >/dev/null
$K -n "$NS" create secret generic recovery --from-literal=recovery-admin="admin:$admin_pass" >/dev/null
t0=$(date +%s)
"$H" install hm deploy/helm/hearth-mail -n "$NS" --wait --timeout 300s \
  --set image.repository="${IMAGE%:*}",image.tag="${IMAGE##*:}",image.pullPolicy=Never \
  --set hostname=mail.hm-test.example,publicUrl=http://127.0.0.1:28380,httpBasicAuth=all \
  --set postgres.host=postgres.db.svc.cluster.local,postgres.passwordSecret.name=pg \
  --set recovery.enabled=true,recovery.adminSecret.name=recovery \
  --set mailService.type=ClusterIP >/dev/null
pass "helm install: server ready in $(( $(date +%s) - t0 ))s"
pod=$($K -n "$NS" get pod -l app.kubernetes.io/component=server -o name | head -1)

# ----------------------------------------------------------------- DNS ----
dig_out=$($K -n "$NS" exec "$pod" -c unbound -- drill -D @127.0.0.1 nlnetlabs.nl A 2>&1 || true)
check 'grep -q "flags:.* ad" <<<"$dig_out"' "Unbound validates: nlnetlabs.nl answered with the AD flag"
bogus=$($K -n "$NS" exec "$pod" -c unbound -- drill @127.0.0.1 dnssec-failed.org A 2>&1 || true)
check 'grep -q "SERVFAIL" <<<"$bogus"' "Unbound refuses a broken signature (dnssec-failed.org: SERVFAIL)"
resolv=$($K -n "$NS" exec "$pod" -c server -- cat /etc/resolv.conf)
check 'grep -q "^nameserver 127.0.0.1" <<<"$resolv" && [ "$(grep -c ^nameserver <<<"$resolv")" = 1 ]' "the pod's only resolver is the sidecar"
logs=$($K -n "$NS" logs "$pod" -c server)
check '! grep -qi "cannot validate DNSSEC\|DANE has been disabled" <<<"$logs"' "the server logs no DNSSEC/DANE warning"
incluster=$($K -n "$NS" exec "$pod" -c unbound -- drill @127.0.0.1 postgres.db.svc.cluster.local A 2>&1 || true)
check 'grep -q "^postgres.db.svc.cluster.local.*IN.*A" <<<"$incluster"' "in-cluster names resolve through the sidecar (the database's)"

# ------------------------------------------------------- API and DANE ----
$K -n "$NS" port-forward "svc/hm-hearth-mail-http" 28380:8080 >/dev/null 2>&1 &
pf_pid=$!
for _ in $(seq 1 30); do curl -fsS -o /dev/null http://127.0.0.1:28380/healthz/ready 2>/dev/null && break; sleep 1; done
code=$(curl -s -o /dev/null -w '%{http_code}' -u "admin:$admin_pass" -X POST -H 'Content-Type: application/json' \
  -d '{"using":["urn:ietf:params:jmap:core","urn:inbuxa:jmap:registry"],"methodCalls":[["x:Domain/get",{"ids":null},"0"]]}' \
  http://127.0.0.1:28380/jmap/)
check '[ "$code" = 200 ]' "the recovery administrator signs in to the API (HTTP $code)"
events=$(curl -s --max-time 60 -u "admin:$admin_pass" "http://127.0.0.1:28380/api/live/delivery/posteo.de?timeout=20" || true)
check 'grep -q "\"tlsaLookupSuccess\"" <<<"$events" && grep -q "\"daneVerifySuccess\"" <<<"$events"' \
  "delivery test to posteo.de: TLSA found and DANE verified"
grep -q '"daneVerifySuccess"' <<<"$events" || grep -o '"type":"[a-zA-Z]*"[^]]*' <<<"$events" | tail -8 || echo "no events: ${events:0:300}"

echo
if [ "$fails" = 0 ]; then echo "all checks passed"; else echo "$fails check(s) failed"; exit 1; fi
