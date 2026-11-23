# Hearth Mail on Kubernetes

The chart in `hearth-mail/` runs one Hearth Mail server with Unbound as its
DNS resolver (a sidecar), and the admin console. PostgreSQL is yours: any
PostgreSQL 14 or later the cluster can reach.

## Install

    kubectl create namespace mail
    kubectl -n mail create secret generic hearth-mail-pg --from-literal=password='<database password>'
    # First start only: a recovery administrator to set things up with
    kubectl -n mail create secret generic hearth-mail-recovery \
      --from-literal=recovery-admin="admin:$(head -c 18 /dev/urandom | base64 | tr -d '/+=')"

    helm install mail deploy/helm/hearth-mail -n mail \
      --set image.repository=<registry>/hearth-mail,image.tag=<commit> \
      --set hostname=mail.example.org,publicUrl=https://mail.example.org \
      --set postgres.host=postgres.db.svc.cluster.local,postgres.passwordSecret.name=hearth-mail-pg \
      --set recovery.enabled=true,recovery.adminSecret.name=hearth-mail-recovery

Sign in to the API or the console with the recovery administrator, create
the domain and the first real administrator, then upgrade with
`recovery.enabled=false` and delete the recovery Secret.

- Mail ports: Service `<release>-hearth-mail-mail`, a LoadBalancer by
  default with `externalTrafficPolicy: Local` (the server must see
  senders' real addresses).
- JMAP, the API and the web pages: Service `<release>-hearth-mail-http`,
  port 8080, for your ingress at `publicUrl`.
- The admin console: Service `<release>-hearth-mail-admin`, port 8080.
- One replica. More need inbuxa's clustering (a coordinator and shared
  blob storage), which this chart doesn't set up.

`values.yaml` lists every setting. The contacts lookup for the spam filter
(docs/integration.md, 1b) is `contactsLookup.url` plus a Secret holding
the shared secret.

## DNS

The pod has one resolver, the Unbound sidecar on 127.0.0.1:53
(`dnsPolicy: None`). It resolves from the root and validates DNSSEC, so
DANE stays on and DNS blocklists answer (deploy/compose/README.md, "Why
Unbound", has the reasons). Names under the cluster domain
(`cluster.local` by default) are forwarded to the cluster DNS, so in-cluster
hosts such as the database still resolve; give them their full name
(`postgres.db.svc.cluster.local`): a name with a dot is first asked as
it is, so a short one costs a needless lookup each time.

The cluster DNS address is read from the `kube-system/kube-dns` Service at
install; set `unbound.clusterDnsIp` where that Service has another name.
An init container seeds Unbound's root key (the two root KSKs IANA
publishes) into an emptyDir; Unbound keeps it current from there.

The pod sets the namespaced sysctl `net.ipv4.ip_unprivileged_port_start=0`
so the server binds ports 25 to 995 and Unbound port 53 without root or
capabilities. Every container runs as non-root with a read-only root file
system and no capabilities.

## Tested

    HM_IMAGE=hearth-mail:<commit> tools/hearth/test-helm.sh

On a throwaway kind cluster it lints the chart, installs it with a test
PostgreSQL, and checks: the server gets ready; Unbound sets the AD flag on
a signed name and refuses a broken one (dnssec-failed.org); the pod's only
resolver is the sidecar; no DNSSEC warning in the server's log; the
database's in-cluster name resolves; the recovery administrator signs in;
the delivery test to posteo.de finds TLSA records and verifies DANE. It
removes the cluster and its tools afterwards.
