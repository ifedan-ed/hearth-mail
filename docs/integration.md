# Integration with Nextcloud and Hearth

Status: sections 1b and 2 (the $itip keyword) are built; the rest is a
plan. Hearth Mail serves mail only; contacts,
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

b. In Hearth Mail itself (optional, for the spam filter). Built.
- The spam filter trusts a sender found in the recipient's contacts
  (`X-Spam-Status: No, reason=card-exists`), as Stalwart did with its own
  address books. The contacts are asked of the cloud that keeps them.
- Settings: `INBUXA_CONTACTS_LOOKUP_URL` and the secret
  (`INBUXA_CONTACTS_LOOKUP_SECRET_FILE` pointing at a secrets file, or
  `INBUXA_CONTACTS_LOOKUP_SECRET`). Both or neither; one alone is a
  warning in the console and the lookup stays off. The spam settings'
  "Trust contacts" switch (`x:SpamSettings.trustContacts`) turns it off
  without removing them; "Learn ham from contacts"
  (`x:SpamClassifier.learnHamFromCard`) trains such mail as ham.
- The contract: `POST {url}`, `Authorization: Bearer <secret>`, body
  `{"account": "<mailbox address>", "sender": "<address>"}`, answer
  `{"known": true}` or `{"known": false}`. `account` is the account's own
  address (its first address, which people sign in with and Tidings
  stores as the account's username), not the alias or masked address the
  mail reached. Code: crates/common/src/contacts_lookup.rs.
- When it's asked: only for mail the filter scored as spam, delivered over
  SMTP or LMTP, whose sender is verified (signed in, or the From domain
  passed DMARC), and not the recipient's own address. Nothing else waits
  on it.
- Never in the way of delivery: one answer per (account, sender) cached
  for 10 minutes; a 300 ms timeout; any failure (timeout, refused, wrong
  status, bad body) means "not known", so the message is scored as it
  would be without the lookup, and one log line says so
  (`spam.classify`, "Contacts lookup failed; scored without it").
- Nextcloud: the Tidings app answers it at
  `https://<cloud>/apps/tidings/api/contacts/known` (branch
  `contacts-known` of Tidings). A mailbox's owners are the users who added
  it as a Tidings account, and the user whose Nextcloud email it is; their
  own and shared address books are searched through Nextcloud's CardDAV
  backend for the exact address. Its secret is set with
  `occ config:app:set tidings contacts_lookup_secret --value=<secret>
  --sensitive --lazy`; without one every call is refused, a wrong one is
  throttled. The same secret goes in Hearth Mail's secrets file.
- Hearth: the same endpoint over its contacts service, when it has one.
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
- Hearth Mail tags every delivered invitation with the keyword `$itip`
  (built; see "The $itip keyword" below), so the apps can list
  invitations with one query and need not read every message's structure.

### The $itip keyword

- Set on mail delivered to a mailbox (SMTP, LMTP) that has a
  `text/calendar` part with a `method` parameter: an iTIP message (RFC
  5546: REQUEST, REPLY, CANCEL, COUNTER and the rest) sent by iMIP (RFC
  6047). A plain `.ics` file attached without `method` is not tagged, nor
  is mail the spam filter files as spam. Code: `is_itip_message` in
  crates/email/src/message/ingest.rs; the name is `ITIP_KEYWORD` in
  crates/types/src/keyword.rs.
- Why this name: `$` keywords are the client-agreed kind (RFC 5788 lists
  `$Forwarded`, `$Junk`, `$MDNSent`...), and "iTIP" is the standard's own
  name for scheduling messages. Nothing else in the registry uses it.
- An ordinary keyword: IMAP shows it as a flag (`FETCH FLAGS`), finds it
  with `SEARCH KEYWORD $itip`, and a client may remove it with
  `STORE -FLAGS ($itip)` (for "handled"); JMAP has it in `keywords` and
  finds it with the filter `{"hasKeyword": "$itip"}`.
- Cheap to search: keyword queries are answered from the per-account
  message cache in memory (one bit mask per message), with no store read
  and no index to maintain. Measured on the throwaway proof: see
  docs/progress.md.
- Tidings and Hearth: list invitations with `Email/query` and
  `{"inMailbox": <inbox>, "hasKeyword": "$itip"}`, read their calendar
  part as now, and may remove the keyword once the person answered, so the
  list holds only open invitations.

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
   people notice first after the removal. Done on the Hearth Mail side:
   the `$itip` keyword.
2. Section 3 in Tidings.
3. Section 1b: built (Hearth Mail and the Tidings endpoint); the Tidings
   branch waits to be merged and its secret set. Section 4 with Hearth's
   phases 2 and 3.
