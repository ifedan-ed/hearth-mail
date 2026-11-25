#!/usr/bin/env python3
"""End-to-end checks beyond smoke.py, on the test bed of tools/hearth/e2e/run.sh.

Run after smoke.py (which bootstraps the instance and leaves the
administrator in $HM_PROOF_DIR/admin). Needs dkimpy, dnspython and
websockets (a venv: `pip install dkimpy pynacl dnspython websockets`).

What it checks, one line each:

- Outbound: submission to an outside address goes through the queue to
  the sink (the MX of remote.example.net, found through the test resolver), over
  STARTTLS (the remote's certificate is self-signed: the first attempt
  refuses it, the retry two minutes later delivers), and both DKIM signatures (RSA and Ed25519) verify against the
  keys the server holds; an address the remote refuses bounces back as a
  delivery report; a vacation reply leaves for an outside sender.
- Inbound authentication: mail from example.net (SPF pass, signed with
  its DKIM key, DMARC pass) lands in the Inbox with an
  Authentication-Results header saying so; mail claiming example.org or the
  server's own domain from an address their SPF doesn't allow goes to Junk
  with dmarc=fail, never refused: the shipped settings check senders
  "relaxed" and the spam filter has no reject score (the maintainer, 2026-11-25:
  "please don't reject, don't want to lose mails").
- Big messages: a 30 MB attachment survives submission, IMAP and JMAP
  byte for byte; a message over the 100 MB SMTP limit is refused.
- Quotas: JMAP Quota/get and IMAP GETQUOTAROOT report the limit and the
  use, also when the account is over it; mail to a full mailbox is held in
  the queue (temporary failure), not stored; IMAP APPEND over quota gets
  NO [OVERQUOTA].
- Search: IMAP SEARCH and JMAP text search find a word in the body of one
  message among 2,000.
- Push: JMAP over WebSocket reports a delivery.
- App passwords: one made by the person signs in to IMAP and SMTP; the
  account password still works; a revoked one doesn't.
- Discovery: Thunderbird autoconfig, Outlook POX Autodiscover.

Exit status 0 when every check passed.
"""
import asyncio
import base64
import hashlib
import imaplib
import json
import os
import secrets
import smtplib
import ssl
import sys
import time
import urllib.error
import urllib.request
from email import message_from_bytes, policy
from email.message import EmailMessage

import dkim
import websockets

DIR = os.environ.get('HM_PROOF_DIR', '/srv/hearth-mail-proof')
HTTP = 'http://127.0.0.1:28080'
HOST = '127.0.0.1'
PORTS = dict(smtp=22525, submissions=22465, submission=22587, imaps=22993)
USING = ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:mail',
         'urn:ietf:params:jmap:submission', 'urn:ietf:params:jmap:vacationresponse',
         'urn:ietf:params:jmap:quota']
REGISTRY = ['urn:ietf:params:jmap:core', 'urn:inbuxa:jmap:registry']

results = []


def check(ok, what, detail=''):
    results.append(bool(ok))
    print(f"{'PASS' if ok else 'FAIL'}  {what}{('  -- ' + str(detail)) if detail and not ok else ''}",
          flush=True)
    return ok


def note(text):
    print(f'      {text}', flush=True)


def tls():
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    return ctx


def http(method, path, body=None, auth=None, headers=None, raw=None):
    data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
    req = urllib.request.Request(HTTP + path, method=method, data=data)
    if body is not None:
        req.add_header('Content-Type', 'application/json')
    if auth:
        req.add_header('Authorization', 'Basic ' + base64.b64encode(f'{auth[0]}:{auth[1]}'.encode()).decode())
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as err:
        return err.code, err.read()


def jmap(auth, calls, using=USING):
    status, body = http('POST', '/jmap/', {'using': using, 'methodCalls': calls}, auth)
    if status != 200:
        raise RuntimeError(f'JMAP {status}: {body[:300]!r}')
    return json.loads(body)['methodResponses']


def wait_for(fn, seconds=60):
    deadline = time.time() + seconds
    while time.time() < deadline:
        value = fn()
        if value:
            return value
        time.sleep(0.5)
    return fn()


def account_id(auth):
    status, body = http('GET', '/jmap/session', auth=auth)
    return json.loads(body)['primaryAccounts']['urn:ietf:params:jmap:mail']


def imap(user):
    m = imaplib.IMAP4_SSL(HOST, PORTS['imaps'], ssl_context=tls())
    m.login(*user)
    return m


def imap_count(user, mailbox='INBOX'):
    with imap(user) as m:
        typ, data = m.select(f'"{mailbox}"', readonly=True)
        return int(data[0]) if typ == 'OK' else -1


def imap_find(user, subject, mailbox='INBOX'):
    """The raw message with this subject in the mailbox, or None."""
    with imap(user) as m:
        if m.select(f'"{mailbox}"', readonly=True)[0] != 'OK':
            return None
        _, data = m.search(None, 'SUBJECT', f'"{subject}"')
        hits = data[0].split()
        if not hits:
            return None
        _, data = m.fetch(hits[-1], '(BODY.PEEK[])')
        return data[0][1]


def submit(user, msg, port='submission'):
    if port == 'submissions':
        with smtplib.SMTP_SSL(HOST, PORTS['submissions'], context=tls(), timeout=300) as s:
            s.login(*user)
            s.send_message(msg)
    else:
        with smtplib.SMTP(HOST, PORTS['submission'], timeout=300) as s:
            s.starttls(context=tls())
            s.login(*user)
            s.send_message(msg)


def message(frm, to, subject, body='Hello from the end-to-end run.'):
    msg = EmailMessage()
    msg['From'], msg['To'], msg['Subject'] = frm, to, subject
    msg['Message-ID'] = f'<{secrets.token_hex(8)}@{frm.split("@")[1]}>'
    msg['Date'] = time.strftime('%a, %d %b %Y %H:%M:%S +0000', time.gmtime())
    msg.set_content(body)
    return msg


# --------------------------------------------------------------- setup ----

def admin_creds():
    return open(f'{DIR}/admin').read().strip().split(':', 1)


def registry(admin, method, args):
    return jmap(admin, [[method, args, '0']], REGISTRY)[0]


def setup(admin):
    """The run's accounts: made once per test bed, then read back from
    $HM_PROOF_DIR/e2e-users.json, so single sections can be run again."""
    if os.path.exists(f'{DIR}/e2e-users.json'):
        return None, {k: tuple(v) for k, v in json.load(open(f'{DIR}/e2e-users.json')).items()}
    domains = {d['name']: d['id'] for d in registry(admin, 'x:Domain/get', {'ids': None})[1]['list']}
    users = {}
    for name, domain in (('erin', 'alpha.test'), ('frank', 'beta.test'), ('gina', 'alpha.test')):
        secret = secrets.token_urlsafe(18)
        r = registry(admin, 'x:Account/set', {'create': {'n': {
            '@type': 'User', 'name': name, 'domainId': domains[domain],
            'description': f'{name.title()} Test',
            'credentials': {'0': {'@type': 'Password', 'secret': secret}}}}})
        made = r[1].get('created', {}).get('n')
        if not made:
            sys.exit(f'create {name}: {json.dumps(r)[:400]}')
        users[name] = (f'{name}@{domain}', secret, made['id'])
    # The ActiveSync replays' two people (tools/activesync/replay.py), in
    # the domain their session files name.
    r = registry(admin, 'x:Domain/set', {'create': {'d': {
        'name': 'example.com', 'certificateManagement': {'@type': 'Manual'},
        'dnsManagement': {'@type': 'Manual'}}}})
    domains['example.com'] = r[1]['created']['d']['id']
    for name, full in (('ann', 'Ann Archer'), ('bob', 'Bob Baker')):
        secret = secrets.token_urlsafe(18)
        r = registry(admin, 'x:Account/set', {'create': {'n': {
            '@type': 'User', 'name': name, 'domainId': domains['example.com'], 'description': full,
            'credentials': {'0': {'@type': 'Password', 'secret': secret}}}}})
        users[name] = (f'{name}@example.com', secret, r[1]['created']['n']['id'])
    return domains, users


# ------------------------------------------------------------ outbound ----

def sink_files():
    return sorted(f for f in os.listdir(f'{DIR}/sink') if f.endswith('.json'))


def sink_wait(subject, seconds=200):
    """The (envelope, raw message) the sink got with this subject."""
    def find():
        for name in sink_files():
            raw = open(f'{DIR}/sink/{name[:-5]}.eml', 'rb').read()
            if f'Subject: {subject}'.encode() in raw:
                return json.load(open(f'{DIR}/sink/{name}')), raw
        return None
    return wait_for(find, seconds)


def dkim_keys(admin):
    """'<selector>._domainkey.<domain>.' -> the TXT record the server
    publishes for it (it would, with real DNS), built from its own keys."""
    domains = {d['id']: d['name'] for d in registry(admin, 'x:Domain/get', {'ids': None})[1]['list']}
    keys = {}
    for sig in registry(admin, 'x:DkimSignature/get', {'ids': None})[1]['list']:
        kind = 'ed25519' if 'Ed25519' in sig['@type'] else 'rsa'
        name = f"{sig['selector']}._domainkey.{domains[sig['domainId']]}."
        keys[name] = f"v=DKIM1; k={kind}; p={sig['publicKey']}".encode()
    return keys, domains


def tls_invalid_certs(admin, allow):
    """Sets allowInvalidCerts on the default outbound TLS strategy."""
    strategies = registry(admin, 'x:MtaTlsStrategy/get', {'ids': None})[1]['list']
    default = next(t['id'] for t in strategies if t['name'] == 'default')
    registry(admin, 'x:MtaTlsStrategy/set', {'update': {default: {'allowInvalidCerts': allow}}})
    time.sleep(2)


def outbound_checks(admin, users):
    erin, frank = users['erin'], users['frank']
    keys, domains = dkim_keys(admin)
    note('DKIM keys held: ' + ', '.join(sorted(keys)))
    subject = f'Outbound {secrets.token_hex(4)}'
    submit(erin[:2], message(erin[0], 'someone@remote.example.net', subject))
    got = sink_wait(subject)
    if not check(got, 'outbound: submission to someone@remote.example.net reaches the remote MX (via MX lookup)'):
        return
    env, raw = got
    check(env['from'] == erin[0] and env['to'] == ['someone@remote.example.net'],
          'outbound: envelope MAIL FROM and RCPT TO as sent', env)
    check(env['ehlo'] == 'mail.alpha.test', 'outbound: EHLO is the mail host name', env['ehlo'])
    check(env['tls'], 'outbound: STARTTLS used to the remote MX (opportunistic, self-signed remote)', env)
    sigs = [line for line in raw.split(b'\r\n') if line.lower().startswith(b'dkim-signature:')]
    verified = dkim.verify(raw, dnsfunc=lambda name, timeout=5: keys.get(name.decode() if isinstance(name, bytes) else name))
    check(len(sigs) >= 2 and verified,
          f'outbound: DKIM-signed ({len(sigs)} signatures) and the signature verifies with the published key',
          f'{len(sigs)} signatures, verify={verified}')
    # Every signature, one by one (dkim.verify checks the first only).
    d = dkim.DKIM(raw)
    ok_each = []
    for i in range(len(sigs)):
        try:
            ok_each.append(d.verify(idx=i, dnsfunc=lambda name, timeout=5: keys.get(
                name.decode() if isinstance(name, bytes) else name)))
        except Exception as err:
            ok_each.append(f'{err}')
    check(all(x is True for x in ok_each), 'outbound: every DKIM signature (RSA and Ed25519) verifies',
          ok_each)

    # The default TLS strategy refuses the sink's self-signed certificate on
    # the first attempt and the retry (the "invalid-tls" strategy) delivers
    # two minutes later, as above. For the rest of the run the default
    # strategy accepts it, so each outbound check doesn't wait two minutes.
    tls_invalid_certs(admin, True)

    # The second domain: was it given DKIM keys when it was created?
    subject = f'Outbound beta {secrets.token_hex(4)}'
    submit(frank[:2], message(frank[0], 'someone@remote.example.net', subject))
    got = sink_wait(subject)
    beta_signed = got and b'd=beta.test' in got[1]
    check(beta_signed, 'outbound: a domain added later (beta.test) signs its mail with its own DKIM key',
          'no DKIM signature with d=beta.test' if got else 'not delivered')

    # A refused address bounces back as a delivery report.
    subject = f'Bounce {secrets.token_hex(4)}'
    submit(erin[:2], message(erin[0], 'reject-me@remote.example.net', subject))
    dsn = wait_for(lambda: imap_find(erin[:2], 'Failed to deliver'), 200)
    check(dsn and subject.encode() in dsn and b'550' in dsn,
          'outbound: a 550 from the remote comes back as a bounce (DSN) with the reason',
          (dsn or b'no DSN in the Inbox')[:300])


def vacation_checks(users):
    gina = users['gina']
    acct = account_id(gina[:2])
    r = jmap(gina[:2], [['VacationResponse/set', {'accountId': acct, 'update': {'singleton': {
        'isEnabled': True, 'subject': 'Away', 'textBody': 'Back on Monday.'}}}, 'v']])
    check('singleton' in (r[0][1].get('updated') or {}), 'JMAP VacationResponse/set turns the auto-reply on',
          json.dumps(r[0])[:300])
    subject = f'Vacation probe {secrets.token_hex(4)}'
    with smtplib.SMTP(HOST, PORTS['smtp'], timeout=30) as s:
        s.ehlo('client.example.net')
        s.send_message(signed(message('pat@example.net', gina[0], subject)),
                       from_addr='pat@example.net')
    # The reply goes to pat@example.net, whose MX is the sink.
    got = sink_wait('Away')
    check(got and got[0]['to'] == ['pat@example.net'] and b'Back on Monday' in got[1],
          'vacation: the auto-reply reaches the outside sender', got[0] if got else 'none')
    jmap(gina[:2], [['VacationResponse/set', {'accountId': acct, 'update': {'singleton': {
        'isEnabled': False}}}, 'v']])


# ------------------------------------------------------------- inbound ----

def signed(msg):
    raw = msg.as_bytes(policy=policy.SMTP)
    key = open(f'{DIR}/keys/outside.key', 'rb').read()
    sig = dkim.sign(raw, b's1', b'example.net', key,
                    include_headers=[b'From', b'To', b'Subject', b'Date', b'Message-ID'])
    return message_from_bytes(sig + raw, policy=policy.SMTP)


def deliver_25(frm, to, msg, helo='client.example.net'):
    """Sends as an outside server on port 25; returns None or the refusal."""
    try:
        with smtplib.SMTP(HOST, PORTS['smtp'], timeout=60) as s:
            s.ehlo(helo)
            if 'STARTTLS' in s.esmtp_features.get('starttls', 'STARTTLS').upper():
                s.starttls(context=tls())
                s.ehlo(helo)
            s.send_message(msg, from_addr=frm, to_addrs=[to])
        return None
    except smtplib.SMTPException as err:
        return err


def inbound_checks(users):
    erin = users['erin']
    subject = f'Signed from outside {secrets.token_hex(4)}'
    msg = signed(message('pat@example.net', erin[0], subject, 'A normal note between colleagues about the agenda.'))
    err = deliver_25('pat@example.net', erin[0], msg)
    check(err is None, 'inbound: signed mail from example.net accepted on 25 (with STARTTLS)', err)
    raw = wait_for(lambda: imap_find(erin[:2], subject))
    if not check(raw, 'inbound: it is filed in the Inbox (not Junk)',
                 'Junk' if imap_find(erin[:2], subject, 'Junk Mail') else 'not found'):
        raw = imap_find(erin[:2], subject, 'Junk Mail') or b''
    head = raw.split(b'\r\n\r\n', 1)[0].decode(errors='replace')
    auth = ' '.join(l for l in head.replace('\r\n\t', ' ').replace('\r\n ', ' ').split('\r\n')
                    if l.lower().startswith('authentication-results'))
    for word in ('spf=pass', 'dkim=pass', 'dmarc=pass', 'iprev=pass'):
        check(word in auth, f'inbound: Authentication-Results has {word}', auth[:400])
    check('Received-SPF: pass' in head, 'inbound: Received-SPF: pass', head[:200])
    spam = [l for l in head.split('\r\n') if l.lower().startswith('x-spam-status')]
    note(f'spam status of the good message: {spam}')

    # DMARC p=reject: example.org's SPF allows nobody here, no signature.
    # The result is recorded and the spam filter scores it
    # (DMARC_POLICY_REJECT): the message goes to Junk, not the Inbox, and is
    # never refused (a refusal in error would lose real mail).
    def spoof(frm, label):
        subject = f'{label} {secrets.token_hex(4)}'
        err = deliver_25(frm, erin[0], message(frm, erin[0], subject))
        junk = wait_for(lambda: imap_find(erin[:2], subject, 'Junk Mail'), 8)
        inbox = imap_find(erin[:2], subject)
        return err, inbox, junk
    err, inbox, junk = spoof('ceo@example.org', 'Spoofed')
    check(err is None and not inbox and junk and b'dmarc=fail' in junk,
          'inbound: mail failing DMARC p=reject goes to Junk with dmarc=fail, not refused',
          f'refused: {err}' if err else f'inbox={bool(inbox)} junk={bool(junk)}')
    err, inbox, junk = spoof('boss@alpha.test', 'Own domain forged')
    check(err is None and not inbox and junk and b'dmarc=fail' in junk,
          "inbound: mail forging the server's own domain from outside goes to Junk, not refused",
          f'refused: {err}' if err else f'inbox={bool(inbox)} junk={bool(junk)}')
    # The shipped setting itself: DMARC checked on the MX port, relaxed,
    # never strict (the maintainer, 2026-11-25: "please don't reject, don't want to
    # lose mails"; docs/decisions.md, "Junk, not refusal").
    admin = admin_creds()
    auth = registry(admin, 'x:SenderAuth/get', {'ids': ['singleton']})[1]['list'][0]
    outcomes = {key: [m['then'] for m in auth[key]['match'].values()] + [auth[key]['else']]
                for key in ('dmarcVerify', 'spfFromVerify', 'spfEhloVerify', 'dkimVerify',
                            'reverseIpVerify', 'arcVerify')}
    check(all(o in ('relaxed', 'disable') for v in outcomes.values() for o in v)
          and 'relaxed' in outcomes['dmarcVerify'],
          'inbound: shipped sender checks are relaxed (Junk, never refused in the session)', outcomes)
    spam = registry(admin, 'x:SpamSettings/get', {'ids': ['singleton']})[1]['list'][0]
    check(not spam.get('scoreReject') and not spam.get('scoreDiscard'),
          'spam filter: shipped with no reject or discard score (spam goes to Junk)', spam)

    # A signed message changed in transit: DKIM fails, SPF still passes,
    # so DMARC passes on SPF (as it should) and the message is accepted.
    subject = f'Tampered {secrets.token_hex(4)}'
    msg = signed(message('pat@example.net', erin[0], subject, 'Original body.'))
    raw = msg.as_bytes(policy=policy.SMTP).replace(b'Original body.', b'Changed body.')
    with smtplib.SMTP(HOST, PORTS['smtp'], timeout=60) as s:
        s.ehlo('client.example.net')
        s.sendmail('pat@example.net', [erin[0]], raw)
    got = wait_for(lambda: imap_find(erin[:2], subject) or imap_find(erin[:2], subject, 'Junk Mail'))
    head = (got or b'').split(b'\r\n\r\n', 1)[0].decode(errors='replace').replace('\r\n\t', ' ')
    # mail-auth reports a body hash mismatch as "neutral (body hash did not
    # verify)"; RFC 6376 6.3 calls it a failure. Either way it is not a pass,
    # which is what DMARC and the spam filter look at.
    check(got and ('dkim=fail' in head or 'dkim=neutral (body hash did not verify)' in head),
          'inbound: a message changed after signing does not pass DKIM', head[:400])


# -------------------------------------------------------- big messages ----

def big_checks(users):
    erin, frank = users['erin'], users['frank']
    data = os.urandom(30 * 1024 * 1024)
    digest = hashlib.sha256(data).hexdigest()
    subject = f'Big attachment {secrets.token_hex(4)}'
    msg = message(erin[0], frank[0], subject, 'A big file attached.')
    msg.add_attachment(data, maintype='application', subtype='octet-stream', filename='big.bin')
    t0 = time.time()
    try:
        submit(erin[:2], msg, 'submissions')
        sent = True
    except smtplib.SMTPException as err:
        sent = err
    check(sent is True, 'big: a 30 MB attachment is accepted on 465', sent)
    note(f'submission of {len(msg.as_bytes()) / 1e6:.1f} MB took {time.time() - t0:.1f} s')
    raw = wait_for(lambda: imap_find(frank[:2], subject), 120)
    got = None
    if raw:
        for part in message_from_bytes(raw, policy=policy.default).iter_attachments():
            got = part.get_content()
    check(got is not None and hashlib.sha256(got).hexdigest() == digest,
          'big: IMAP FETCH returns the attachment byte for byte')
    # JMAP: the attachment's blob downloads whole.
    acct = account_id(frank[:2])
    r = jmap(frank[:2], [['Email/query', {'accountId': acct, 'filter': {'subject': subject}}, 'q'],
                         ['Email/get', {'accountId': acct, '#ids': {'resultOf': 'q', 'name': 'Email/query',
                                                                    'path': '/ids'},
                                        'properties': ['attachments']}, 'g']])
    blobs = [a['blobId'] for e in r[1][1]['list'] for a in e['attachments']]
    ok = False
    if blobs:
        t0 = time.time()
        status, body = http('GET', f'/jmap/download/{acct}/{blobs[0]}/big.bin?accept=application/octet-stream',
                            auth=frank[:2])
        ok = status == 200 and hashlib.sha256(body).hexdigest() == digest
        note(f'JMAP download of 30 MB took {time.time() - t0:.1f} s')
    check(ok, 'big: JMAP download of the attachment blob is byte for byte')

    # Over the SMTP limit (100 MB): refused, not half-stored.
    huge = message(erin[0], frank[0], 'Too big')
    huge.add_attachment(os.urandom(80 * 1024 * 1024), maintype='application', subtype='octet-stream',
                        filename='huge.bin')
    try:
        submit(erin[:2], huge, 'submissions')
        refused = None
    except (smtplib.SMTPException, OSError) as err:
        refused = err
    check(refused is not None, 'big: a 108 MB message (80 MB base64-encoded) is refused at submission',
          'accepted')
    note(f'refusal: {str(refused)[:200]}')


# --------------------------------------------------------------- quota ----

def quota_checks(admin, users):
    frank = users['frank']
    r = registry(admin, 'x:Account/set', {'update': {frank[2]: {'quotas': {'maxDiskQuota': 40 * 1024 * 1024}}}})
    check(frank[2] in (r[1].get('updated') or {}), 'quota: 40 MB disk quota set on frank', json.dumps(r)[:300])
    time.sleep(2)
    acct = account_id(frank[:2])
    r = jmap(frank[:2], [['Quota/get', {'accountId': acct}, 'q']])
    quotas = r[0][1].get('list', [])
    octets = [q for q in quotas if q.get('resourceType') == 'octets']
    check(octets and octets[0].get('hardLimit') == 40 * 1024 * 1024,
          'quota: JMAP Quota/get reports the limit and use', json.dumps(r[0])[:300])
    if octets:
        note(f"used {octets[0].get('used', 0) / 1e6:.1f} MB of {octets[0]['hardLimit'] / 1e6:.0f} MB")
    # The same limit over IMAP (RFC 9208), in KiB.
    with imap(frank[:2]) as m:
        typ, data = m.getquotaroot('INBOX')
    text = b' '.join(x if isinstance(x, bytes) else b' '.join(x) for x in data[1] if x).decode()
    check(f'STORAGE {octets[0]["used"] // 1024 if octets else 0} {40 * 1024}' in text,
          'quota: IMAP GETQUOTAROOT reports the same limit (40960 KiB)', text)
    # frank holds the 30 MB message and is over his 40 MB: a 15 MB message
    # from outside is accepted in the session (the recipient check doesn't
    # look at quotas, as upstream), then held in the queue with a temporary
    # "mailbox full" and retried, never stored over the limit. The sender
    # gets a delay notice after a day and a bounce when it expires.
    subject = f'Over quota {secrets.token_hex(4)}'
    msg = message('pat@example.net', frank[0], subject)
    msg.add_attachment(os.urandom(11 * 1024 * 1024), maintype='application', subtype='octet-stream',
                       filename='x.bin')
    err = deliver_25('pat@example.net', frank[0], signed(msg))
    time.sleep(3)
    queued = registry(admin, 'x:QueuedMessage/query', {})[1].get('ids', [])
    held = []
    if queued:
        got = registry(admin, 'x:QueuedMessage/get', {'ids': queued})[1].get('list', [])
        held = [q for q in got if frank[0] in json.dumps(q)]
    check(err is None and not imap_find(frank[:2], subject) and held,
          'quota: mail to a full mailbox is not stored and stays in the queue (temporary failure)',
          f'session: {err}; stored: {bool(imap_find(frank[:2], subject))}; queued: {len(held)}')
    for q in held:
        registry(admin, 'x:QueuedMessage/set', {'destroy': [q['id']]})
    with imap(frank[:2]) as m:
        typ, data = m.append('INBOX', None, None, b'Subject: big\r\n\r\n' + b'x' * (12 * 1024 * 1024))
    check(typ == 'NO' and b'QUOTA' in b' '.join(data).upper(), 'quota: IMAP APPEND over quota gets NO [OVERQUOTA]',
          f'{typ} {data}')
    registry(admin, 'x:Account/set', {'update': {frank[2]: {'quotas': {}}}})


# -------------------------------------------------------------- search ----

def search_checks(users):
    gina = users['gina']
    word = 'zephyrquokka' + secrets.token_hex(2)
    with imap(gina[:2]) as m:
        m.create('Load')
        t0 = time.time()
        for i in range(2000):
            body = f'Routine report number {i}.' + (f' The {word} figures are in.' if i == 1234 else '')
            m.append('Load', None, None, (f'From: r{i}@example.net\r\nTo: {gina[0]}\r\n'
                                          f'Subject: report {i}\r\nMessage-ID: <r{i}@example.net>\r\n\r\n'
                                          f'{body}\r\n').encode())
        note(f'2,000 IMAP APPENDs took {time.time() - t0:.1f} s')

    def imap_hits():
        with imap(gina[:2]) as m:
            m.select('Load', readonly=True)
            t0 = time.perf_counter()
            _, data = m.search(None, 'BODY', word)
            return data[0].split(), (time.perf_counter() - t0) * 1000
    hits, ms = wait_for(lambda: (lambda h: h if h[0] else None)(imap_hits()), 60) or ([], 0)
    check(len(hits) == 1, 'search: IMAP SEARCH BODY finds the one message among 2,000', hits)
    note(f'IMAP SEARCH BODY: {ms:.0f} ms')
    acct = account_id(gina[:2])
    t0 = time.perf_counter()
    r = jmap(gina[:2], [['Email/query', {'accountId': acct, 'filter': {'text': word}}, 'q']])
    ms = (time.perf_counter() - t0) * 1000
    check(len(r[0][1].get('ids', [])) == 1, 'search: JMAP Email/query text finds it', json.dumps(r[0])[:200])
    note(f'JMAP text search: {ms:.0f} ms')
    with imap(gina[:2]) as m:
        m.select('Load', readonly=True)
        _, data = m.search(None, 'SUBJECT', '"report 1999"')
    check(len(data[0].split()) == 1, 'search: IMAP SEARCH SUBJECT', data)


# ---------------------------------------------------------------- push ----

def websocket_checks(users):
    erin = users['erin']
    auth = 'Basic ' + base64.b64encode(f'{erin[0]}:{erin[1]}'.encode()).decode()

    async def run():
        async with websockets.connect('ws://127.0.0.1:28080/jmap/ws', subprotocols=['jmap'],
                                      additional_headers={'Authorization': auth}) as ws:
            await ws.send(json.dumps({'@type': 'WebSocketPushEnable', 'dataTypes': None}))
            await asyncio.sleep(1)
            await asyncio.get_running_loop().run_in_executor(
                None, submit, users['gina'][:2], message(users['gina'][0], erin[0], 'WebSocket push'))
            while True:
                frame = json.loads(await asyncio.wait_for(ws.recv(), 30))
                if frame.get('@type') == 'StateChange' and 'Email' in json.dumps(frame):
                    return frame
    try:
        frame = asyncio.run(run())
    except Exception as err:
        frame = None
        note(f'websocket: {err!r}')
    check(frame, 'push: JMAP WebSocket reports the delivery (StateChange for Email)')


# ------------------------------------------------------- app passwords ----

def app_password_checks(admin, users):
    erin = users['erin']
    r = jmap(erin[:2], [['x:AppPassword/set', {'create': {'p': {'description': 'Phone'}}}, '0']], REGISTRY)
    made = r[0][1].get('created', {}).get('p')
    if not check(made and made.get('secret'), 'app password: the person makes one (x:AppPassword/set)',
                 json.dumps(r[0])[:300]):
        return
    app = (erin[0], made['secret'])
    try:
        with imap(app) as m:
            ok_imap = m.select('INBOX', readonly=True)[0] == 'OK'
    except imaplib.IMAP4.error as err:
        ok_imap = err
    check(ok_imap is True, 'app password: IMAP sign-in', ok_imap)
    try:
        submit(app, message(erin[0], users['gina'][0], 'Sent with an app password'))
        ok_smtp = True
    except smtplib.SMTPException as err:
        ok_smtp = err
    check(ok_smtp is True, 'app password: SMTP submission sign-in', ok_smtp)
    status, _ = http('GET', '/jmap/session', auth=app)
    check(status == 200, 'app password: JMAP (Basic) sign-in', status)
    jmap(erin[:2], [['x:AppPassword/set', {'destroy': [made['id']]}, '0']], REGISTRY)
    time.sleep(1)
    try:
        with imap(app):
            still = True
    except imaplib.IMAP4.error:
        still = False
    check(not still, 'app password: a revoked one no longer signs in')


# ------------------------------------------------------------ discovery ----

def discovery_checks(users):
    erin = users['erin']
    status, body = http('GET', f'/mail/config-v1.1.xml?emailaddress={erin[0]}')
    check(status == 200 and b'<incomingServer type="imap">' in body and b'mail.alpha.test' in body,
          'discovery: Thunderbird autoconfig (/mail/config-v1.1.xml)', f'{status} {body[:200]!r}')
    status, body = http('GET', f'/.well-known/autoconfig/mail/config-v1.1.xml?emailaddress={erin[0]}')
    check(status == 200 and b'imap' in body, 'discovery: /.well-known/autoconfig', status)
    pox = ('<?xml version="1.0" encoding="utf-8"?><Autodiscover xmlns="http://schemas.microsoft.com/'
           'exchange/autodiscover/outlook/requestschema/2006"><Request><EMailAddress>'
           f'{erin[0]}</EMailAddress><AcceptableResponseSchema>http://schemas.microsoft.com/exchange/'
           'autodiscover/outlook/responseschema/2006a</AcceptableResponseSchema></Request></Autodiscover>')
    status, body = http('POST', '/autodiscover/autodiscover.xml', raw=pox.encode(),
                        headers={'Content-Type': 'text/xml'})
    check(status == 200 and b'IMAP' in body and b'SMTP' in body, 'discovery: Outlook POX Autodiscover',
          f'{status} {body[:200]!r}')


def main():
    admin = admin_creds()
    _, users = setup(admin)
    with open(f'{DIR}/e2e-users.json', 'w') as f:
        os.chmod(f'{DIR}/e2e-users.json', 0o600)
        json.dump(users, f)
    sections = [
        ('outbound', lambda: outbound_checks(admin, users)),
        ('inbound', lambda: inbound_checks(users)),
        ('vacation', lambda: vacation_checks(users)),
        ('app passwords', lambda: app_password_checks(admin, users)),
        ('discovery', lambda: discovery_checks(users)),
        ('push', lambda: websocket_checks(users)),
        ('search', lambda: search_checks(users)),
        ('big', lambda: big_checks(users)),
        ('quota', lambda: quota_checks(admin, users)),
    ]
    only = set(sys.argv[1:])
    for name, run in sections:
        if only and name not in only:
            continue
        print(f'-- {name}', flush=True)
        try:
            run()
        except Exception as err:
            check(False, f'{name}: the section ran to the end', repr(err)[:400])
    tls_invalid_certs(admin, False)
    passed = sum(results)
    print(f'\n{passed} of {len(results)} checks passed')
    sys.exit(0 if passed == len(results) else 1)


if __name__ == '__main__':
    main()
