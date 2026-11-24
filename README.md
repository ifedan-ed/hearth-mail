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
- A spam filter with rules, DNS blocklists and a learned model. Mail from
  a sender in the recipient's contacts (in Nextcloud or Hearth) can skip
  the spam folder.
- Meeting invitations are tagged with the `$itip` keyword, so any client
  can list them (IMAP `SEARCH KEYWORD $itip`, JMAP `hasKeyword`).
- Its own validating DNS resolver in the Compose and Helm examples, for
  DNSSEC, DANE and DNS blocklists.
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
settings by themselves from the email address (autoconfig, Autodiscover in
its POX, SOAP and JSON forms, and SRV records). JMAP clients use `https://<host>/.well-known/jmap`.

Exchange ActiveSync (14.1 and 16.1) is served at
`https://<host>/Microsoft-Server-ActiveSync`: iPhone Mail, Outlook for iOS
and Android, and Android's Exchange apps get mail with push, and, through
the cloud, calendars, contacts, tasks and notes, in one account set up
from the address and the password. `https://<host>/setup` gives iPhones
and Macs a profile with the mail account and the cloud's calendars and
contacts. Autodiscover answers in its POX, SOAP and JSON forms, so
Outlook and phones find the server from the address alone.
docs/activesync.md has the details.

Calendars, contacts, tasks and notes are not stored in Hearth Mail: the
DAV bridge reads and writes them in the cloud over CalDAV and CardDAV
(settings below), so the same items show in Nextcloud and any DAV client.
Without the bridge, ActiveSync serves mail only.

Tested with replayed sessions of iPhone Mail, Outlook mobile and an
Android client, written from Microsoft's protocol documents, and against a
throwaway Nextcloud 35. Not yet tried with real devices.

## Running

Two ready examples, each with PostgreSQL, the admin console and Unbound as
the server's DNS resolver:

- Docker Compose: deploy/compose (start with its README).
- Kubernetes: the Helm chart in deploy/helm/hearth-mail (deploy/helm/README.md).

The container image runs as user 2000 and works with a read-only root:

    docker run -d --name hearth-mail --read-only --tmpfs /tmp \
      -v "$PWD/etc:/etc/inbuxa" -v "$PWD/data:/var/lib/inbuxa" \
      -p 25:25 -p 465:465 -p 587:587 -p 993:993 -p 995:995 -p 4190:4190 \
      -p 443:443 -p 8080:8080 \
      hearth-mail:<version>

Settings are read from `INBUXA_*` environment variables and from the store
named in `/etc/inbuxa/config.json`. The first start without a store opens a
setup page on port 8080.

Environment settings added by Hearth Mail:

- `INBUXA_CONTACTS_LOOKUP_URL` and `INBUXA_CONTACTS_LOOKUP_SECRET` (or
  `INBUXA_CONTACTS_LOOKUP_SECRET_FILE`): where the spam filter asks whether
  a sender is in the recipient's contacts. Both or neither. With Nextcloud
  this is the Tidings app's `/apps/tidings/api/contacts/known`
  (docs/integration.md, section 1b). The spam settings' "Trust contacts"
  switch turns the check off.

- `INBUXA_DAV_URL` and `INBUXA_DAV_AUTH` (`passthrough` or
  `app-password`): the cloud's CalDAV/CardDAV root
  (`https://<cloud>/remote.php/dav` for Nextcloud) for ActiveSync's
  calendars, contacts, tasks and notes, and how Hearth Mail signs in there:
  with the device's own password, or with an app password each person links
  on `/setup`. Both or neither; without them ActiveSync serves mail only.

The admin console is a separate program,
[inbuxa Admin](https://git.coffeylabs.org/inbuxa/inbuxa-admin). It builds its
pages from the server's schema, so it shows only what Hearth Mail does.

## Building

Builds run in a pinned toolchain container, with memory and CPU capped:

    ci/cargo.sh build -p inbuxa --locked        # a dev build
    ci/build-image.sh                           # the release image

Tests: while working, run only the tests of what changed, the crate's own
and the integration suites that touch it, for example:

    ci/cargo.sh test -p activesync-proto -p activesync -p dav-client
    STORE=RocksDb ci/cargo.sh test -p tests activesync -- --test-threads=1
    tools/activesync/test-bridge-nextcloud.sh   # ActiveSync with a throwaway Nextcloud

The full suite (every crate's unit tests and the integration suites) runs
on GitHub Actions, on demand and before a release or a switch of a live
server; every push there runs formatting, clippy, the build and the
changed crates' tests. docs/building.md has the details, and how the public
copy on GitHub is made.

## Documentation

- docs/integration.md: contacts, calendars, files and administration with
  Nextcloud and Hearth.
- docs/activesync.md: Exchange ActiveSync, Autodiscover and the device setup page.
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
