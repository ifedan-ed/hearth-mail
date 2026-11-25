#!/usr/bin/env python3
"""Concurrent clients against the end-to-end test bed (tools/hearth/e2e/run.sh).

Run after e2e.py (it reads the accounts e2e.py left in
$HM_PROOF_DIR/e2e-users.json). For LOAD_SECONDS (default 60), at once:

- LOAD_SENDERS (10) submission clients on 587 with STARTTLS, each sending
  one message after another to the three accounts;
- LOAD_IMAP (20) IMAP clients, each signed in once and looping SELECT,
  UID FETCH of the newest 50 headers, SEARCH UNSEEN, NOOP;
- LOAD_JMAP (10) JMAP clients looping Email/query plus Email/get of 50.

Every client connects from one address (the test bed's gateway), so the
shipped inbound throttles (5 connections a second per address, 25
messages an hour from a sender domain to one recipient) would refuse most
of the mail: they are switched off for the run (administrator API) and
back on afterwards. The HTTP limit (1,000 requests a minute per person)
stays: a JMAP request it refuses (429) is counted as throttled, not as an
error, and the client waits a second, as clients do.

Then it checks that every accepted message was delivered (each one's
subject found in its recipient's Inbox), that no client saw an error, and prints
the throughput and the latency percentiles of each kind of request. Exit
status 0 when nothing was lost and no request failed.
"""
import base64
import imaplib
import secrets
import json
import os
import smtplib
import ssl
import statistics
import threading
import time
import urllib.error
import urllib.request
from email.message import EmailMessage

DIR = os.environ.get('HM_PROOF_DIR', '/srv/hearth-mail-proof')
HOST = '127.0.0.1'
SECONDS = int(os.environ.get('LOAD_SECONDS', '60'))
SENDERS = int(os.environ.get('LOAD_SENDERS', '10'))
IMAPS = int(os.environ.get('LOAD_IMAP', '20'))
JMAPS = int(os.environ.get('LOAD_JMAP', '10'))

users = {k: tuple(v[:2]) for k, v in json.load(open(f'{DIR}/e2e-users.json')).items()}
names = sorted(users)
lock = threading.Lock()
timings = {}
errors = []
throttled = [0]
sent = {n: set() for n in names}
RUN = secrets.token_hex(3)
stop = time.time() + SECONDS


def record(kind, started):
    with lock:
        timings.setdefault(kind, []).append((time.perf_counter() - started) * 1000)


def tls():
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    return ctx


def sender(i):
    """Sends one message after another. The server takes ten messages per
    SMTP session (maxMessages), so the client starts a new session every
    ten, as mail programs and relays do."""
    me = users[names[i % len(names)]]
    k = 0
    try:
        while time.time() < stop:
            s = smtplib.SMTP(HOST, 22587, timeout=60)
            s.starttls(context=tls())
            s.login(*me)
            for _ in range(10):
                if time.time() >= stop:
                    break
                to = names[(i + k) % len(names)]
                msg = EmailMessage()
                msg['From'], msg['To'] = me[0], users[to][0]
                msg['Subject'] = f'load {RUN} {i}-{k}'
                msg.set_content(f'Load message {k} from sender {i}.\n' + 'Lorem ipsum dolor sit amet. ' * 40)
                t0 = time.perf_counter()
                s.send_message(msg)
                record('SMTP send', t0)
                with lock:
                    sent[to].add(msg['Subject'])
                k += 1
            s.quit()
    except Exception as err:
        with lock:
            errors.append(f'sender {i}: {err!r}')


def delivered(user):
    """The subjects of this run's load messages in the user's Inbox."""
    m = imaplib.IMAP4_SSL(HOST, 22993, ssl_context=tls())
    m.login(*user)
    m.select('INBOX', readonly=True)
    _, data = m.search(None, 'SUBJECT', f'"load {RUN} "')
    found = set()
    ids = data[0].split()
    for start in range(0, len(ids), 500):
        _, rows = m.fetch(b','.join(ids[start:start + 500]), '(BODY.PEEK[HEADER.FIELDS (SUBJECT)])')
        for row in rows:
            if isinstance(row, tuple):
                found.add(row[1].decode().split(':', 1)[1].strip())
    m.logout()
    return found


def imap_client(i):
    me = users[names[i % len(names)]]
    try:
        m = imaplib.IMAP4_SSL(HOST, 22993, ssl_context=tls())
        m.login(*me)
        while time.time() < stop:
            t0 = time.perf_counter()
            typ, data = m.select('INBOX', readonly=True)
            record('IMAP SELECT', t0)
            n = int(data[0])
            if n:
                t0 = time.perf_counter()
                typ, _ = m.fetch(f'{max(1, n - 49)}:{n}', '(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE)])')
                record('IMAP FETCH 50 headers', t0)
                assert typ == 'OK'
            t0 = time.perf_counter()
            typ, _ = m.search(None, 'UNSEEN')
            record('IMAP SEARCH UNSEEN', t0)
            assert typ == 'OK'
            m.noop()
        m.logout()
    except Exception as err:
        with lock:
            errors.append(f'imap {i}: {err!r}')


def jmap_client(i):
    me = users[names[i % len(names)]]
    auth = 'Basic ' + base64.b64encode(f'{me[0]}:{me[1]}'.encode()).decode()

    def post(body):
        req = urllib.request.Request('http://127.0.0.1:28080/jmap/', data=json.dumps(body).encode(),
                                     headers={'Authorization': auth, 'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read())
    try:
        req = urllib.request.Request('http://127.0.0.1:28080/jmap/session', headers={'Authorization': auth})
        acct = json.loads(urllib.request.urlopen(req, timeout=60).read())['primaryAccounts']['urn:ietf:params:jmap:mail']
        while time.time() < stop:
            t0 = time.perf_counter()
            try:
                r = post({'using': ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:mail'], 'methodCalls': [
                ['Email/query', {'accountId': acct, 'sort': [{'property': 'receivedAt', 'isAscending': False}],
                                 'limit': 50}, 'q'],
                ['Email/get', {'accountId': acct, '#ids': {'resultOf': 'q', 'name': 'Email/query', 'path': '/ids'},
                               'properties': ['subject', 'from', 'receivedAt', 'preview', 'keywords']}, 'g']]})
            except urllib.error.HTTPError as err:
                if err.code != 429:
                    raise
                with lock:
                    throttled[0] += 1
                time.sleep(1)
                continue
            record('JMAP query+get 50', t0)
            assert r['methodResponses'][1][0] == 'Email/get', r
    except Exception as err:
        with lock:
            errors.append(f'jmap {i}: {err!r}')


def admin_call(method, args):
    admin = open(f'{DIR}/admin').read().strip().split(':', 1)
    auth = 'Basic ' + base64.b64encode(f'{admin[0]}:{admin[1]}'.encode()).decode()
    req = urllib.request.Request('http://127.0.0.1:28080/jmap/', data=json.dumps({
        'using': ['urn:ietf:params:jmap:core', 'urn:inbuxa:jmap:registry'],
        'methodCalls': [[method, args, '0']]}).encode(),
        headers={'Authorization': auth, 'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read())['methodResponses'][0][1]


def throttles(enable):
    """Switches the inbound SMTP throttles on or off."""
    for t in admin_call('x:MtaInboundThrottle/get', {'ids': None})['list']:
        admin_call('x:MtaInboundThrottle/set', {'update': {t['id']: {'enable': enable}}})


def main():
    throttles(False)
    time.sleep(2)
    try:
        run()
    finally:
        throttles(True)


def run():
    threads = [threading.Thread(target=sender, args=(i,)) for i in range(SENDERS)]
    threads += [threading.Thread(target=imap_client, args=(i,)) for i in range(IMAPS)]
    threads += [threading.Thread(target=jmap_client, args=(i,)) for i in range(JMAPS)]
    t0 = time.time()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    elapsed = time.time() - t0
    total = sum(len(v) for v in sent.values())
    # Delivery is asynchronous (the queue): give it time to drain.
    deadline = time.time() + 120
    while True:
        missing = {n: sent[n] - delivered(users[n]) for n in names}
        missing = {n: v for n, v in missing.items() if v}
        if not missing or time.time() > deadline:
            break
        time.sleep(2)
    print(f'{SENDERS} senders, {IMAPS} IMAP and {JMAPS} JMAP clients for {elapsed:.0f} s')
    print(f'{total} messages sent ({total / elapsed:.1f}/s)')
    for kind, ms in sorted(timings.items()):
        ms.sort()
        p = lambda q: ms[min(len(ms) - 1, int(len(ms) * q))]
        print(f'  {kind:24} n={len(ms):6}  median {statistics.median(ms):7.1f} ms  '
              f'p95 {p(0.95):7.1f} ms  max {ms[-1]:7.1f} ms')
    print(f'  JMAP requests refused by the per-person limit (429): {throttled[0]}')
    ok = not errors and not missing and total > 0
    print(f"{'PASS' if not missing and total else 'FAIL'}  every accepted message delivered to its Inbox"
          + (f'  -- missing: { {n: len(v) for n, v in missing.items()} }' if missing else ''))
    print(f"{'PASS' if not errors else 'FAIL'}  no client saw an error"
          + (f'  -- {errors[:5]}' if errors else ''))
    raise SystemExit(0 if ok else 1)


if __name__ == '__main__':
    main()
