# Hearth Mail

The mail server of the Hearth suite. It runs on its own, for any standard
mail client, and works with Nextcloud and Hearth for contacts, calendars and
files.

Hearth Mail is a fork of [inbuxa](https://git.coffeylabs.org/inbuxa/inbuxa-server)
27.1-rc.39, itself a fork of Stalwart, with calendars, contacts and files
taken out. It is maintained on its own from that point.

## What it does

- JMAP for mail, with push (EventSource, WebSocket, Web Push).
- IMAP4rev2 and IMAP4rev1, POP3.
- SMTP: receiving on port 25, submission on 465 and 587, outbound delivery
  with queues, DANE, MTA-STS and REQUIRETLS.
- DKIM, SPF, DMARC and ARC checks and signing; DMARC and TLS reports, sent
  and received.
- Sieve filters and vacation replies, managed over ManageSieve or JMAP.
- A spam filter with rules, DNS blocklists and a learned model.
- Many users, groups, shared mailboxes, domains and tenants; mailing lists
  and masked addresses.
- Sign-in with passwords, app passwords, passkeys, OAuth and OpenID
  Connect providers.
- Undelete for mail and Sieve scripts, legal holds, account locks with
  delegates, mail rules and data loss prevention, an audit log.
- Automatic certificates (ACME) and DNS records for its domains.
- A management API, used by the admin console and by scripts.

What it does not do: calendars (CalDAV, JMAP Calendars), contacts (CardDAV,
JMAP Contacts) and files (WebDAV, JMAP FileNode). Use Nextcloud or Hearth
for those; docs/integration.md shows how they fit together.

## Mail clients

Any client that speaks IMAP and SMTP works: Thunderbird, Apple Mail,
Outlook, K-9 and the other phone apps. Thunderbird and Outlook find the
settings by themselves from the email address (autoconfig, autodiscover and
SRV records). JMAP clients use `https://<host>/.well-known/jmap`.

## Running

The container image runs as user 2000 and works with a read-only root:

    docker run -d --name hearth-mail --read-only --tmpfs /tmp \
      -v "$PWD/etc:/etc/inbuxa" -v "$PWD/data:/var/lib/inbuxa" \
      -p 25:25 -p 465:465 -p 587:587 -p 993:993 -p 995:995 -p 4190:4190 \
      -p 443:443 -p 8080:8080 \
      hearth-mail:<version>

Settings are read from `INBUXA_*` environment variables and from the store
named in `/etc/inbuxa/config.json`. The first start without a store opens a
setup page on port 8080.

The admin console is a separate program,
[inbuxa Admin](https://git.coffeylabs.org/inbuxa/inbuxa-admin). It builds its
pages from the server's schema, so it shows only what Hearth Mail does.

## Building

Builds run in a pinned toolchain container, with memory and CPU capped:

    ci/cargo.sh build -p inbuxa --locked        # a dev build
    ci/build-image.sh                           # the release image

Tests:

    ci/cargo.sh test --workspace --exclude tests --locked --features smtp/test_mode
    STORE=RocksDb ci/cargo.sh test -p tests --locked -- --test-threads=1

docs/building.md has the details.

## Documentation

- docs/integration.md: contacts, calendars, files and administration with
  Nextcloud and Hearth.
- docs/building.md: builds, images, tests and CI.
- docs/spec/: inbuxa's own feature specifications, kept for the features
  that remain.

## License and credits

Free software under the [GNU Affero General Public License, version
3](./LICENSES/AGPL-3.0-only.txt).

Hearth Mail is a fork of inbuxa (Coffey Labs), which started as a fork of
Stalwart (Stalwart Labs LLC). Their copyright notices stay on every file
they cover. A few files carry code from other projects under MIT or BSD
licenses, which stays under those licenses; THIRD-PARTY.md lists it with its
notices. Stalwart is a trademark of Stalwart Labs LLC. Hearth Mail is not
affiliated with Stalwart Labs or Coffey Labs.
