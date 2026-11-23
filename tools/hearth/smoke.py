#!/usr/bin/env python3
"""End-to-end check of a throwaway Hearth Mail instance.

Run after `tools/hearth/throwaway.sh up IMAGE`. Sets the instance up through
its own API (bootstrap into Postgres, two domains, four users), then uses it
the way mail clients do and prints one line per check:

  SMTP in (port 25) across both domains, submission on 465 and 587 with
  sign-in, IMAP as Thunderbird uses it (STARTTLS and TLS, CAPABILITY, ID,
  LIST, SELECT, FETCH, APPEND to Sent, MOVE, IDLE seeing new mail), POP3,
  ManageSieve and a Sieve rule filing mail, the spam filter sending GTUBE to
  Junk, JMAP mail (session, query, get, set, EmailSubmission) and JMAP push
  (an EventSource reporting a delivery), the management API the console
  uses, the console itself, and that calendars, contacts and files are gone.

Exit status 0 when every check passed. Throwaway passwords are made here
and printed nowhere.
"""
import base64
import gzip
import imaplib
import json
import os
import poplib
import secrets
import smtplib
import socket
import ssl
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from email.message import EmailMessage

DIR = os.environ.get('HM_PROOF_DIR', '/srv/hearth-mail-proof')
HTTP = 'http://127.0.0.1:28080'
CONSOLE = 'http://127.0.0.1:28081'
HOST = '127.0.0.1'
PORTS = dict(smtp=22525, submissions=22465, submission=22587, imap=22143, imaps=22993,
             pop3s=22995, sieve=24190)
DOMAINS = ('alpha.test', 'beta.test')
USERS = {'alice': 'alpha.test', 'bob': 'alpha.test', 'carol': 'beta.test', 'dave': 'beta.test'}
JMAP_USING = ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:mail',
              'urn:ietf:params:jmap:submission']

results = []


def check(ok, what, detail=''):
    results.append(bool(ok))
    print(f"{'PASS' if ok else 'FAIL'}  {what}{('  -- ' + detail) if detail and not ok else ''}")
    return ok


def tls():
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    return ctx


def http(method, path, body=None, auth=None, headers=None, base=HTTP):
    req = urllib.request.Request(base + path, method=method,
                                 data=json.dumps(body).encode() if body is not None else None)
    if body is not None:
        req.add_header('Content-Type', 'application/json')
    if auth:
        req.add_header('Authorization', 'Basic ' + base64.b64encode(f'{auth[0]}:{auth[1]}'.encode()).decode())
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as err:
        return err.code, err.read()


def jmap(auth, calls, using=JMAP_USING):
    status, body = http('POST', '/jmap/', {'using': using, 'methodCalls': calls}, auth)
    if status != 200:
        raise RuntimeError(f'JMAP {status}: {body[:300]!r}')
    return json.loads(body)['methodResponses']


def one(auth, method, args, using=JMAP_USING + ['urn:inbuxa:jmap:registry']):
    return jmap(auth, [[method, args, '0']], using)[0]


def wait_http(seconds=180):
    for _ in range(seconds):
        try:
            urllib.request.urlopen(HTTP + '/healthz/live', timeout=2)
            return True
        except urllib.error.HTTPError:
            return True
        except Exception:
            time.sleep(1)
    return False


def docker(*args):
    return subprocess.run(['docker', *args], capture_output=True, text=True)


# --------------------------------------------------------------- setup ----

def setup():
    recovery = open(f'{DIR}/recovery-admin').read().strip().split(':', 1)
    pg_secret = open(f'{DIR}/pg-password').read().strip()
    if not wait_http():
        sys.exit('server did not answer:\n' + docker('logs', '--tail', '40', 'hmproof-server').stderr)
    got = one(recovery, 'x:Bootstrap/get', {'ids': None})
    sid = got[1]['list'][0]['id']
    res = one(recovery, 'x:Bootstrap/set', {'update': {sid: {
        'serverHostname': 'mail.alpha.test', 'defaultDomain': 'alpha.test',
        'requestTlsCertificate': False, 'generateDkimKeys': True,
        'dataStore': {'@type': 'PostgreSql', 'host': 'hmproof-pg', 'port': 5432,
                      'database': 'inbuxa', 'authUsername': 'inbuxa',
                      'authSecret': {'@type': 'Value', 'secret': pg_secret}},
        'tracer': {'@type': 'Stdout', 'level': 'info', 'ansi': False},
    }}})
    upd = res[1].get('updated', {}).get(sid)
    if not upd:
        sys.exit('bootstrap failed: ' + json.dumps(res)[:500])
    admin = (upd['username'], upd['secret'])
    check(True, 'bootstrap into Postgres (the live store type)')
    docker('restart', 'hmproof-server')
    if not wait_http():
        sys.exit('server did not come back after bootstrap')
    time.sleep(3)

    def create(kind, obj):
        r = one(admin, f'x:{kind}/set', {'create': {'n': obj}})
        made = r[1].get('created', {}).get('n')
        if not made:
            sys.exit(f'create {kind} failed: {json.dumps(r)[:400]}')
        return made['id']

    domains = {}
    for d in one(admin, 'x:Domain/get', {'ids': None})[1]['list']:
        domains[d['name']] = d['id']
    if 'beta.test' not in domains:
        domains['beta.test'] = create('Domain', {
            'name': 'beta.test', 'certificateManagement': {'@type': 'Manual'},
            'dnsManagement': {'@type': 'Manual'}})
    check(set(DOMAINS) <= set(domains), 'two domains', str(domains))
    users = {}
    for name, domain in USERS.items():
        secret = secrets.token_urlsafe(18)
        create('Account', {'@type': 'User', 'name': name, 'domainId': domains[domain],
                           'description': f'{name.title()} Test',
                           'credentials': {'0': {'@type': 'Password', 'secret': secret}}})
        users[name] = (f'{name}@{domain}', secret)
    check(len(users) == 4, 'four users across both domains')
    # The bootstrap opens 25, 465, 993, 995 and 4190; the live server also
    # has submission with STARTTLS on 587 and IMAP with STARTTLS on 143,
    # which Thunderbird and phones offer first. Added as an operator would.
    for name, port, protocol in (('submission', 587, 'smtp'), ('imap', 143, 'imap')):
        create('NetworkListener', {'name': name, 'bind': {f'[::]:{port}': True},
                                   'protocol': protocol, 'tlsImplicit': False})
    docker('restart', 'hmproof-server')
    if not wait_http():
        sys.exit('server did not come back after adding listeners')
    time.sleep(3)
    return admin, users


# -------------------------------------------------------------- checks ----

def message(frm, to, subject, body='Hello from the throwaway test.'):
    msg = EmailMessage()
    msg['From'], msg['To'], msg['Subject'] = frm, to, subject
    msg['Message-ID'] = f'<{secrets.token_hex(8)}@test.example>'
    msg.set_content(body)
    return msg


def smtp_checks(users):
    with smtplib.SMTP(HOST, PORTS['smtp'], timeout=30) as s:
        s.ehlo('outside.example')
        s.send_message(message('someone@outside.example', users['alice'][0], 'Inbound to alpha'))
        s.send_message(message('someone@outside.example', users['carol'][0], 'Inbound to beta'))
    check(True, 'SMTP in on 25: accepted for alice@alpha.test and carol@beta.test')
    with smtplib.SMTP(HOST, PORTS['smtp'], timeout=30) as s:
        try:
            s.send_message(message('someone@outside.example', 'nobody@alpha.test', 'x'))
            check(False, 'SMTP in refuses an unknown recipient')
        except smtplib.SMTPRecipientsRefused:
            check(True, 'SMTP in refuses an unknown recipient')
    with smtplib.SMTP_SSL(HOST, PORTS['submissions'], context=tls(), timeout=30) as s:
        s.login(*users['alice'])
        s.send_message(message(users['alice'][0], users['carol'][0], 'Alice to Carol over 465'))
    check(True, 'submission on 465 (TLS) with sign-in: alpha -> beta')
    with smtplib.SMTP(HOST, PORTS['submission'], timeout=30) as s:
        s.starttls(context=tls())
        s.login(*users['dave'])
        s.send_message(message(users['dave'][0], users['bob'][0], 'Dave to Bob over 587'))
    check(True, 'submission on 587 (STARTTLS) with sign-in: beta -> alpha')
    with smtplib.SMTP(HOST, PORTS['submission'], timeout=30) as s:
        s.starttls(context=tls())
        try:
            s.login(users['dave'][0], 'wrong password')
            check(False, 'submission refuses a wrong password')
        except smtplib.SMTPAuthenticationError:
            check(True, 'submission refuses a wrong password')


def submit(sender, to, subject, body='Hello from the throwaway test.'):
    """Sends as a signed-in user over 465: mail between the instance's own
    people, which the spam filter treats as ham (mail from outside.example
    has no SPF, DMARC or reverse DNS in this sandbox, so it scores as
    spam and goes to Junk, as it should)."""
    with smtplib.SMTP_SSL(HOST, PORTS['submissions'], context=tls(), timeout=30) as s:
        s.login(*sender)
        s.send_message(message(sender[0], to, subject, body))


def wait_for(fn, seconds=30):
    for _ in range(seconds * 2):
        value = fn()
        if value:
            return value
        time.sleep(0.5)
    return fn()


def imap_count(user, mailbox='INBOX'):
    with imaplib.IMAP4_SSL(HOST, PORTS['imaps'], ssl_context=tls()) as m:
        m.login(*user)
        typ, data = m.select(f'"{mailbox}"', readonly=True)
        return int(data[0]) if typ == 'OK' else -1


def imap_checks(users):
    carol = users['carol']
    # Thunderbird's opening: STARTTLS on 143, CAPABILITY, ID, AUTHENTICATE PLAIN
    m = imaplib.IMAP4(HOST, PORTS['imap'])
    m.starttls(ssl_context=tls())
    caps = m.capabilities
    m.authenticate('PLAIN', lambda _: f'\0{carol[0]}\0{carol[1]}'.encode())
    typ, data = m.capability()
    caps = data[0].decode().split()
    check('IDLE' in caps and 'MOVE' in caps and 'UIDPLUS' in caps and 'CONDSTORE' in caps,
          'IMAP STARTTLS on 143, AUTHENTICATE PLAIN; IDLE, MOVE, UIDPLUS, CONDSTORE offered',
          str(caps))
    typ, data = m.xatom('ID', '("name" "Thunderbird" "version" "140.0")')
    check(typ == 'OK', 'IMAP ID (as Thunderbird sends it)')
    typ, boxes = m.list()
    names = b' '.join(boxes)
    check(typ == 'OK' and b'Sent' in names and b'Junk' in names and b'Trash' in names,
          'IMAP LIST shows the special folders')
    got = wait_for(lambda: imap_count(carol) >= 1 and imap_count(carol, 'Junk Mail') >= 1)
    check(got, 'IMAP: carol has the submitted message in INBOX and the outside one in Junk',
          f'INBOX {imap_count(carol)}, Junk {imap_count(carol, "Junk Mail")}')
    m.select('INBOX')
    typ, data = m.uid('SEARCH', None, 'ALL')
    uids = data[0].split()
    typ, data = m.uid('FETCH', uids[-1], '(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT)])')
    check(typ == 'OK' and b'Subject:' in data[0][1], 'IMAP UID FETCH of headers')
    sent = next(b.decode().rsplit(' "/" ', 1)[1].strip('"') for b in boxes if b'\\Sent' in b)
    msg = message(carol[0], users['alice'][0], 'Copy kept by the client')
    typ, _ = m.append(f'"{sent}"', '(\\Seen)', imaplib.Time2Internaldate(time.time()), msg.as_bytes())
    check(typ == 'OK' and imap_count(carol, sent) == 1, f'IMAP APPEND to the \\Sent folder ({sent})')
    m.create('Archive')
    typ, _ = m.uid('MOVE', uids[0], 'Archive')
    check(typ == 'OK' and imap_count(carol, 'Archive') == 1, 'IMAP MOVE to a new folder')
    # IDLE: a new delivery shows up as EXISTS without polling
    m.select('INBOX')
    tag = m._new_tag()
    m.send(tag + b' IDLE\r\n')
    m.readline()  # + idling
    seen = {}

    def deliver():
        time.sleep(2)
        submit(users['alice'], carol[0], 'Arrives during IDLE')
    threading.Thread(target=deliver).start()
    m.sock.settimeout(30)
    try:
        while True:
            line = m.readline()
            if b'EXISTS' in line:
                seen['exists'] = line
                break
    except socket.timeout:
        pass
    m.send(b'DONE\r\n')
    m.readline()
    check('exists' in seen, 'IMAP IDLE reports new mail (EXISTS) as it arrives')
    m.logout()
    with imaplib.IMAP4_SSL(HOST, PORTS['imaps'], ssl_context=tls()) as m2:
        try:
            m2.login(carol[0], 'wrong')
            check(False, 'IMAP refuses a wrong password')
        except imaplib.IMAP4.error:
            check(True, 'IMAP refuses a wrong password')


def pop3_checks(users):
    p = poplib.POP3_SSL(HOST, PORTS['pop3s'], context=tls())
    p.user(users['bob'][0])
    p.pass_(users['bob'][1])
    count, _ = p.stat()
    p.quit()
    check(count >= 1, 'POP3 over TLS: sign-in and STAT', f'{count} messages')


def sieve_checks(users):
    bob = users['bob']
    script = ('require ["fileinto", "mailbox", "imap4flags"];\r\n'
              'if header :contains "subject" "[list]" {\r\n'
              '  addflag "$list"; fileinto :create "Lists";\r\n}\r\n')
    raw = socket.create_connection((HOST, PORTS['sieve']), timeout=30)

    def read_until_ok(f):
        lines = []
        while True:
            line = f.readline()
            lines.append(line)
            if line.startswith((b'OK', b'NO', b'BYE')):
                return lines

    f = raw.makefile('rb')
    banner = read_until_ok(f)
    has_starttls = any(b'STARTTLS' in l for l in banner)
    raw.sendall(b'STARTTLS\r\n')
    read_until_ok(f)
    s = tls().wrap_socket(raw, server_hostname='mail.alpha.test')
    f = s.makefile('rb')
    read_until_ok(f)  # capabilities again after TLS
    auth = base64.b64encode(f'\0{bob[0]}\0{bob[1]}'.encode()).decode()
    s.sendall(f'AUTHENTICATE "PLAIN" "{auth}"\r\n'.encode())
    ok_auth = read_until_ok(f)[-1].startswith(b'OK')
    body = script.encode()
    s.sendall(b'PUTSCRIPT "lists" {%d+}\r\n' % len(body) + body + b'\r\n')
    ok_put = read_until_ok(f)[-1].startswith(b'OK')
    s.sendall(b'SETACTIVE "lists"\r\n')
    ok_active = read_until_ok(f)[-1].startswith(b'OK')
    s.sendall(b'LISTSCRIPTS\r\n')
    listing = b''.join(read_until_ok(f))
    s.sendall(b'LOGOUT\r\n')
    s.close()
    check(has_starttls and ok_auth and ok_put and ok_active and b'"lists" ACTIVE' in listing,
          'ManageSieve: STARTTLS, AUTHENTICATE, PUTSCRIPT, SETACTIVE')
    submit(users['dave'], bob[0], '[list] Weekly digest')
    filed = wait_for(lambda: imap_count(bob, 'Lists') == 1)
    check(filed, 'Sieve rule files matching mail into a new folder (fileinto :create)')


def spam_checks(users):
    bob = users['bob']
    gtube = 'XJS*C4JDBQADN1.NSBN3*2IDNEN*GTUBE-STANDARD-ANTI-UBE-TEST-EMAIL*C.34X'
    with smtplib.SMTP(HOST, PORTS['smtp'], timeout=30) as s:
        s.send_message(message('spammer@outside.example', bob[0], 'Buy now', gtube))
    junk = wait_for(lambda: imap_count(bob, 'Junk Mail') >= 1)
    status = b''
    if junk:
        with imaplib.IMAP4_SSL(HOST, PORTS['imaps'], ssl_context=tls()) as m:
            m.login(*bob)
            m.select('"Junk Mail"', readonly=True)
            _, data = m.fetch(b'1', '(BODY.PEEK[HEADER.FIELDS (X-SPAM-STATUS)])')
            status = data[0][1]
    check(junk and b'Yes' in status, 'spam filter: GTUBE goes to Junk with X-Spam-Status: Yes',
          status.decode(errors='replace').strip())


def jmap_checks(users):
    alice = users['alice']
    status, body = http('GET', '/jmap/session', auth=alice)
    session = json.loads(body)
    caps = set(session.get('capabilities', {}))
    check(status == 200 and 'urn:ietf:params:jmap:mail' in caps, 'JMAP session')
    gone = {c for c in caps if any(w in c for w in ('calendars', 'contacts', 'filenode',
                                                    'availability'))}
    check(not gone, 'JMAP session offers no calendars, contacts or files', str(gone))
    account = session['primaryAccounts']['urn:ietf:params:jmap:mail']
    r = jmap(alice, [['Email/query', {'accountId': account, 'sort': [{'property': 'receivedAt',
                                                                      'isAscending': False}]}, 'q'],
                     ['Email/get', {'accountId': account, '#ids': {'resultOf': 'q',
                      'name': 'Email/query', 'path': '/ids'}, 'properties': ['subject', 'keywords']}, 'g']])
    subjects = [e['subject'] for e in r[1][1]['list']]
    check('Inbound to alpha' in subjects, 'JMAP Email/query and Email/get', str(subjects))
    first = r[1][1]['list'][0]['id']
    r = jmap(alice, [['Email/set', {'accountId': account,
                                    'update': {first: {'keywords/$flagged': True}}}, 's']])
    check(first in (r[0][1].get('updated') or {}), 'JMAP Email/set (flag a message)')

    # Push: an EventSource stays open and reports the next delivery
    events = []

    def listen():
        req = urllib.request.Request(HTTP + '/jmap/eventsource/?types=*&closeafter=state&ping=0')
        req.add_header('Authorization', 'Basic ' + base64.b64encode(f'{alice[0]}:{alice[1]}'.encode()).decode())
        req.add_header('Accept', 'text/event-stream')
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                for line in resp:
                    events.append(line)
                    if line.startswith(b'data:') and b'Email' in line:
                        return
        except Exception as err:
            events.append(repr(err).encode())
    t = threading.Thread(target=listen)
    t.start()
    time.sleep(2)
    submit(users['carol'], alice[0], 'For the push test')
    t.join(30)
    check(any(l.startswith(b'data:') and b'Email' in l for l in events),
          'JMAP push: EventSource reports the delivery (StateChange for Email)',
          b''.join(events)[:200].decode(errors='replace'))

    # Sending over JMAP
    ident = jmap(alice, [['Identity/get', {'accountId': account}, 'i']])[0][1]['list'][0]['id']
    drafts = [b for b in jmap(alice, [['Mailbox/get', {'accountId': account}, 'm']])[0][1]['list']]
    drafts_id = next(b['id'] for b in drafts if b.get('role') == 'drafts')
    r = jmap(alice, [
        ['Email/set', {'accountId': account, 'create': {'d': {
            'mailboxIds': {drafts_id: True}, 'keywords': {'$draft': True},
            'from': [{'email': alice[0]}], 'to': [{'email': users['dave'][0]}],
            'subject': 'Sent over JMAP', 'textBody': [{'partId': '1', 'type': 'text/plain'}],
            'bodyValues': {'1': {'value': 'Hello Dave'}}}}}, 'e'],
        ['EmailSubmission/set', {'accountId': account, 'create': {'s': {
            'identityId': ident, 'emailId': '#d'}}}, 'sub']],
        JMAP_USING)
    check(r[1][1].get('created', {}).get('s'), 'JMAP EmailSubmission sends a message',
          json.dumps(r[1])[:300])
    check(wait_for(lambda: imap_count(users['dave']) >= 1), 'the JMAP-sent message reaches dave@beta.test')

    # A client that lists the removed capabilities still gets its mail
    # calls answered; a calendar method itself is unknown
    status, body = http('POST', '/jmap/', {'using': JMAP_USING + ['urn:ietf:params:jmap:calendars',
                                                                 'urn:ietf:params:jmap:contacts'],
                                           'methodCalls': [['Mailbox/get', {'accountId': account}, 'm'],
                                                           ['Calendar/get', {'accountId': account}, 'c']]},
                        alice)
    answers = json.loads(body).get('methodResponses', []) if status == 200 else []
    check(len(answers) == 2 and answers[0][0] == 'Mailbox/get' and answers[1][0] == 'error'
          and answers[1][1].get('type') == 'unknownMethod',
          'JMAP: listing calendars/contacts in "using" still works for mail; Calendar/get is unknownMethod',
          f'{status} {body[:200]!r}')


def removed_checks(users):
    for path in ('/dav/cal/', '/dav/card/', '/dav/file/', '/.well-known/caldav',
                 '/.well-known/carddav', '/calendar/rsvp'):
        status, _ = http('PROPFIND' if path.startswith('/dav') else 'GET', path, auth=users['alice'])
        check(status in (404, 405), f'gone: {path}', str(status))
    status, body = http('GET', '/.well-known/source')
    check(status == 200 and b'github.com/ifedan-ed/hearth-mail' in body,
          '/.well-known/source names this repository', body[:200].decode(errors='replace'))


def admin_checks(admin, users):
    r = one(admin, 'x:Account/query', {})
    check(len(r[1].get('ids', [])) >= 5, 'management API: x:Account/query', str(r[1])[:200])
    r = one(admin, 'x:Domain/get', {'ids': None})
    names = sorted(d['name'] for d in r[1]['list'])
    check(names == sorted(DOMAINS), 'management API: x:Domain/get', str(names))
    r = one(admin, 'x:QueuedMessage/query', {})
    check(r[0] == 'x:QueuedMessage/query', 'management API: the queue')
    status, body = http('GET', '/api/schema', auth=admin)
    if body[:2] == b'\x1f\x8b':  # served with Content-Encoding: gzip
        body = gzip.decompress(body)
    schema = json.loads(body) if status == 200 else {}
    objs = set(schema.get('objects', {}))
    check(status == 200 and 'x:Account' in objs, 'console schema served at /api/schema')
    leftovers = {o for o in objs if any(w in o for w in ('Calendar', 'AddressBook', 'WebDav',
                                                       'FileStorage'))}
    check(not leftovers, 'console schema has no calendar, contact, file or WebDAV objects',
          str(leftovers))
    status, body = http('GET', '/', base=CONSOLE)
    check(status == 200 and b'<html' in body.lower(), 'the admin console is served')


def main():
    admin, users = setup()
    smtp_checks(users)
    imap_checks(users)
    pop3_checks(users)
    sieve_checks(users)
    spam_checks(users)
    jmap_checks(users)
    removed_checks(users)
    admin_checks(admin, users)
    # For the console screenshot (tools/hearth/console-shot.mjs), kept in the
    # proof directory only, which `throwaway.sh down` removes.
    with open(f'{DIR}/admin', 'w') as f:
        os.chmod(f'{DIR}/admin', 0o600)
        f.write(f'{admin[0]}:{admin[1]}')
    passed = sum(results)
    print(f'\n{passed} of {len(results)} checks passed')
    sys.exit(0 if passed == len(results) else 1)


if __name__ == '__main__':
    main()
