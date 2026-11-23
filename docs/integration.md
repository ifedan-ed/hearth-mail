# Integration with Nextcloud and Hearth

Status: plan, nothing built. Hearth Mail serves mail only; contacts,
calendars and files live in Nextcloud (cloud.example.org) now and in Hearth
later. This page says how the pieces meet. Each part works with Hearth Mail
alone too: a standard mail client never needs any of it.

The rule for every part: Hearth Mail exposes standard protocols (JMAP, IMAP,
SMTP, ManageSieve) and its management API; the cloud side (Tidings in
Nextcloud, Hearth's mail screens) does the joining. Hearth Mail itself calls
out to the cloud in one place only (the contacts lookup in section 1b), and
only when it's configured to.

## Who talks to whom

    phone / Thunderbird / Apple Mail ──IMAP, SMTP, JMAP──▶ Hearth Mail
              └──────────CalDAV, CardDAV──────────▶ Nextcloud or Hearth

    Tidings (in Nextcloud) ──JMAP + push (OAuth)──▶ Hearth Mail
              └──OCP Contacts / Calendar / Files (in-process)──▶ Nextcloud

    Hearth (later) ──JMAP + push, management API──▶ Hearth Mail
              └──its own contacts, calendar, files

## 1. Contacts for recipient autocomplete

a. In the mail apps (no server change)
- Tidings already does this: `lib/Service/ContactsService.php` searches the
  user's own Nextcloud address books through `OCP\Contacts\IManager`
  (FN, EMAIL, NICKNAME, ORG; the system address book left out), and saves
  senders to them. Nothing changes.
- Hearth's mail screens (roadmap phase 3) ask Hearth's contacts service
  directly, in-process, the same way; there is no CardDAV hop inside Hearth.
- Standard clients use their own CardDAV account against Nextcloud or
  Hearth (phones, DAVx5, Thunderbird's address book). Hearth Mail's
  Thunderbird autoconfig can point them there (section 5).
- Recently used addresses not in any address book: Tidings and Hearth can
  add them from the user's Sent mail (JMAP `Email/query` on the Sent
  mailbox, `to`/`cc` properties), cached on their side.

b. In Hearth Mail itself (optional, for the spam filter)
- The spam filter used to trust a sender found in the recipient's own
  address book (`card-exists`). To bring it back without a contact store
  here: one setting, the URL of a contacts lookup service
  (`x:SpamSettings`, a new `contactsLookupUrl`), off when empty.
- The lookup is `POST {url}` with `{"account": "<address>", "sender":
  "<address>"}` and a service token, answering `{"known": true|false}`;
  Hearth implements it over its contacts, and a small Nextcloud app (or a
  Tidings endpoint, which already has `ContactsService`) over Nextcloud's.
  One answer per (account, sender) cached for 10 minutes; a 300 ms
  timeout; any failure means "not known" (the message is scored as now,
  never trusted by mistake).
- Why not CardDAV from Hearth Mail: it would need every user's app
  password stored here and an address-book query per incoming message.

## 2. Calendar invitations

Since the removal, an invitation reaching Hearth Mail is delivered as the
email it is, its `text/calendar; method=REQUEST` (or REPLY, CANCEL) part
untouched. Nothing is written to any calendar by the mail server.

- Standard clients (Apple Mail with Calendar, Thunderbird, phones) already
  act on such mail against their own CalDAV calendar.
- Tidings: when a message has a `text/calendar` part (JMAP `Email/get`
  with `bodyStructure` and the part's blob), show the event with Accept,
  Maybe, Decline. On a choice:
  1. write it into the user's Nextcloud calendar with
     `OCP\Calendar\IManager::handleIMipRequest` / `handleIMipReply` /
     `handleIMipCancel` (Nextcloud's own iMIP handling, which matches the
     UID and updates an existing event);
  2. send the iTIP reply (`METHOD:REPLY`, the attendee's PARTSTAT) to the
     organizer as a mail through JMAP `EmailSubmission/set`, from the
     identity the invitation was addressed to.
  Replies arriving for events the user organised go through
  `handleIMipReply` the same way, which updates the attendee's status.
- Hearth (phase 2 calendar, phase 3 mail): the same flow against Hearth's
  calendar service, in Go; the parsing and the reply builder are shared by
  the mail screens and Hearth's calendar (which sends its own invitations
  through Hearth Mail's SMTP submission or JMAP).
- Optional help from Hearth Mail: a Sieve rule (`header :contains
  "Content-Type" "text/calendar"` → `addflag "$calendar"`) could tag such
  mail so the apps find it with a keyword query. Open question in
  docs/decisions.md.

## 3. Attachments from and to files

- Attach from files. Tidings: the Nextcloud file picker, then the server
  reads the file (`OCP\Files\IRootFolder`) and uploads it to Hearth Mail
  (`POST /jmap/upload/{accountId}/`), so the bytes never go through the
  browser; the blob id goes into the draft's `attachments`. Over the
  attachment size limit, a Nextcloud share link goes into the body
  instead. Hearth: the same from its file service, server to server.
- Save to files. JMAP `GET /jmap/download/{accountId}/{blobId}/{name}`
  from the server side, written into the chosen folder.
- Hearth Mail needs no change for either: uploads and downloads are
  standard JMAP, already limited by its upload quota and size settings.

## 4. One admin view in Hearth (optional)

- Hearth's admin screens read and change Hearth Mail through its
  management API (the same JMAP `x:` objects the console uses), with an API
  key kept in Hearth's secrets: accounts and their mail quotas, domains,
  the queue, delivery and DMARC reports, the weekly digest.
- Creating a person in Hearth can create the mail account (`x:Account/set`)
  in the same step, and removing one removes it (Hearth Mail's own
  account removal does the rest).
- Everything else stays in Hearth Mail's console, linked from Hearth: it is
  complete and already maintained with the server.
- Sessions per person (list, sign out one device) need upstream's #435
  (OAuth grant registry), on our list in docs/decisions.md.

## 5. Discovery

So that clients set up with only an email address find all three:

- Mail: Hearth Mail's autoconfig, autodiscover, SRV and MTA-STS records, as
  now.
- Calendars and contacts: Hearth Mail's Thunderbird autoconfig can list the
  cloud's CalDAV and CardDAV (`<addressBook>`, `<calendar>` entries), and
  `/.well-known/caldav` and `/.well-known/carddav` on the mail host can
  redirect to it. One setting, the cloud's DAV base URL
  (`https://cloud.example.org/remote.php/dav/` for Nextcloud), off when
  empty. The SRV records `_caldavs._tcp` and `_carddavs._tcp` would then
  point at the cloud's host, written by the automatic DNS the same way.

## 6. Sign-in and push

- Tidings signs in with OAuth (client `tidings-cloud-example-org`) and keeps
  the refresh token; nothing changes.
- Hearth (phase 3, roadmap): signing in to Hearth opens the mailbox. Hearth
  is an OAuth client of Hearth Mail like Tidings; the first time, the
  person authorises it once; afterwards Hearth refreshes the token. Later
  both can trust the same Zitadel (Hearth Mail supports OIDC providers) so
  even that one step goes.
- Push: Hearth keeps one JMAP EventSource (or WebSocket) per signed-in
  person and forwards state changes on its own event stream to every tab;
  Tidings keeps its own.

## Order

1. Section 5 (discovery) and 2 (invitations in Tidings): the two things
   people notice first after the removal.
2. Section 3 in Tidings.
3. Sections 1b and 4 with Hearth's phases 2 and 3.
