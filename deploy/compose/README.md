# Hearth Mail with Docker Compose

Four services: the mail server, PostgreSQL for its store, Unbound as its
DNS resolver, and the admin console.

## Start

    cp .env.example .env              # then fill it in
    mkdir -p secrets etc data
    chmod 700 secrets
    head -c 24 /dev/urandom | base64 | tr -d '/+=' > secrets/pg-password
    chown 2000:2000 etc data          # the server runs as user 2000
    docker compose up -d
    docker compose logs server        # the one-time setup password

The first start opens a setup page on port 8080 (127.0.0.1 only; reach it
through your reverse proxy or an SSH tunnel). Sign in as `admin` with the
password from the log, choose PostgreSQL with host `postgres`, database and
user `inbuxa` and the password in `secrets/pg-password`, and set up the
first domain and administrator. The server restarts on its own.

Point a reverse proxy at port 8080 for `HEARTH_MAIL_PUBLIC_URL`, and at
8081 for the admin console. The mail ports (25, 465, 587, 143, 993, 995,
4190) are published directly.

## Why Unbound

A mail server needs a DNS resolver that does three things the usual ones
don't:

- Validates DNSSEC. DANE (TLSA records, used to check other servers'
  certificates) is only safe on validated answers; without a validating
  resolver the server logs "cannot validate DNSSEC" and turns DANE off.
  Docker's own resolver (127.0.0.11) and many hosting resolvers don't
  validate.
- Answers DNS blocklists. Spamhaus and the other lists refuse queries
  from big public resolvers (Google, Cloudflare, Quad9), so a server using
  them gets no blocklist answers or wrong ones.
- Passes long answers. Docker's resolver answers SERVFAIL when a reply is
  too long for UDP (TXT and TLSA records often are). Hearth Mail retries
  those over TCP, but a resolver of its own avoids the problem.

Unbound here resolves from the root itself, validates, and keeps the root
key up to date (RFC 5011, in the `unbound` volume). Only the server uses
it: it listens on its own network (`dns`, 172.30.53.0/24 by default) and
on no host port. The server's `dns:` entry sends every lookup to it.

Check it:

    docker compose exec unbound drill -D @127.0.0.1 nlnetlabs.nl   # flags include "ad"
    docker compose exec unbound drill @127.0.0.1 dnssec-failed.org # SERVFAIL: broken signature refused
    docker compose logs server | grep -i dnssec                    # nothing: DANE is on

The console's delivery test (Troubleshoot, any domain with TLSA records,
for example posteo.de) shows "DANE verified".

If 172.30.53.0/24 is taken on the host, set `HEARTH_MAIL_DNS_SUBNET` and
`HEARTH_MAIL_DNS_IP` in `.env`.

## Contacts for the spam filter (optional)

To let mail from people in the recipient's contacts skip the spam folder
(docs/integration.md, 1b): put the shared secret in
`secrets/contacts-lookup-secret`, and in `docker-compose.yml` uncomment
`INBUXA_CONTACTS_LOOKUP_URL`, `INBUXA_CONTACTS_LOOKUP_SECRET_FILE` and the
secret's volume line. With Nextcloud the URL is
`https://<cloud>/apps/tidings/api/contacts/known`, and the same secret is
set in Tidings with `occ config:app:set tidings contacts_lookup_secret
--value=<secret> --sensitive --lazy`.

## Tested

`tools/hearth/smoke.py` runs against this example (the IMAP, SMTP, POP3,
JMAP, Sieve and admin checks, and with `HM_PROOF_DNS=1` the DNSSEC and
DANE checks through Unbound); docs/progress.md has the latest run.
