#!/usr/bin/env python3
"""Replays the ActiveSync device sessions against a running server.

The same session files the `tests` crate replays against its in-process
test server (tests/resources/activesync/*.txt, the format is described in
tests/src/activesync/replay.rs), sent here to a real instance over HTTPS,
the way the devices would reach a deployed image: iPhone Mail (16.1, plain
query), an Android Exchange client (14.1, base64 query) and Outlook mobile
(16.1).

    tools/activesync/replay.py https://127.0.0.1:28443 ann@example.com:SECRET \\
        bob@example.com:SECRET [iphone.txt android.txt outlook.txt]

The first account replays iphone.txt and android.txt, the second
outlook.txt, as in the suite. Both must exist on the server and be able to
sign in with Basic. `!deliver` puts a message into the account's Inbox
through JMAP (Email/import, as the suite does). nextcloud.txt (the DAV
bridge) needs INBUXA_DAV_URL on the server and the same accounts in the
cloud: pass DAV_URL=<the cloud's DAV root> in the environment to run it.

WBXML is encoded and decoded here from the code page table the server
itself uses (crates/activesync-proto/src/wbxml/codepages.rs, generated
from [MS-ASWBXML]), so the two can't disagree on a token. Standard library
only.
"""
import base64
import json
import os
import re
import ssl
import sys
import threading
import time
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
RESOURCES = os.path.join(ROOT, 'tests/resources/activesync')

# ------------------------------------------------------------- code pages ----


def load_pages():
    """[(namespace, {token: name}, {name: token})] from the Rust table."""
    text = open(os.path.join(ROOT, 'crates/activesync-proto/src/wbxml/codepages.rs')).read()
    table = text[text.index('pub static PAGES'):]
    pages = []
    for block in re.finditer(r'namespace: "(\w+)",\s*tags: &\[(.*?)\]', table, re.S):
        tags = {int(t, 16): n for t, n in re.findall(r'\(0x([0-9A-Fa-f]{2}), "(\w+)"\)', block.group(2))}
        pages.append((block.group(1), tags, {n: t for t, n in tags.items()}))
    assert len(pages) == 26, len(pages)
    return pages


PAGES = load_pages()
ALIASES = {'POOMCONTACTS': 'Contacts', 'POOMMAIL': 'Email', 'POOMCAL': 'Calendar',
           'POOMTASKS': 'Tasks', 'POOMCONTACTS2': 'Contacts2', 'POOMMAIL2': 'Email2'}


def page_of(namespace):
    name = namespace.rstrip(':')
    name = ALIASES.get(name.upper(), name)
    for i, (ns, _, _) in enumerate(PAGES):
        if ns.lower() == name.lower():
            return i
    raise ValueError(f'unknown namespace {namespace!r}')


# ------------------------------------------------------------------ model ----
# An element: [page, name, content], content None, str, bytes or a list.

def from_xml(text):
    root = ET.fromstring(text)

    def walk(node):
        ns, name = node.tag[1:].split('}', 1) if node.tag.startswith('{') else ('AirSync:', node.tag)
        page = page_of(ns)
        if name not in PAGES[page][2]:
            raise ValueError(f'unknown tag {ns}{name}')
        kids = list(node)
        if kids:
            return [page, name, [walk(k) for k in kids]]
        txt = node.text
        if node.get('opaque') == 'base64':
            return [page, name, base64.b64decode(txt or '')]
        return [page, name, txt if txt is not None and txt != '' else None]
    return walk(root)


def to_xml(el, depth=0):
    page, name, content = el
    pad = '  ' * depth
    tag = f'{PAGES[page][0]}:{name}'
    if content is None:
        return f'{pad}<{tag}/>'
    if isinstance(content, list):
        inner = '\n'.join(to_xml(c, depth + 1) for c in content)
        return f'{pad}<{tag}>\n{inner}\n{pad}</{tag}>'
    if isinstance(content, bytes):
        try:
            content = content.decode()
        except UnicodeDecodeError:
            return f'{pad}<{tag} opaque="base64">{base64.b64encode(content).decode()}</{tag}>'
    shown = content if len(content) < 400 else content[:400] + '...'
    return f'{pad}<{tag}>{shown}</{tag}>'


def mb_u32(n):
    out = [n & 0x7F]
    n >>= 7
    while n:
        out.insert(0, (n & 0x7F) | 0x80)
        n >>= 7
    return bytes(out)


def encode(root):
    out = bytearray(b'\x03\x01\x6a\x00')
    state = {'page': 0}

    def write(el):
        page, name, content = el
        if page != state['page']:
            out.extend((0x00, page))
            state['page'] = page
        token = PAGES[page][2][name]
        if content is None or content == []:
            out.append(token)
        elif isinstance(content, list):
            out.append(token | 0x40)
            for c in content:
                write(c)
            out.append(0x01)
        elif isinstance(content, bytes) or '\0' in content:
            data = content if isinstance(content, bytes) else content.encode()
            out.append(token | 0x40)
            out.append(0xC3)
            out.extend(mb_u32(len(data)))
            out.extend(data)
            out.append(0x01)
        else:
            out.append(token | 0x40)
            out.append(0x03)
            out.extend(content.encode())
            out.append(0)
            out.append(0x01)
    write(root)
    return bytes(out)


def decode(data):
    if len(data) < 4 or data[2] != 0x6A:
        raise ValueError(f'not ActiveSync WBXML: {data[:16]!r}')
    pos, page = 4, 0
    stack, root = [], None

    def mb():
        nonlocal pos
        n = 0
        while True:
            b = data[pos]
            pos += 1
            n = (n << 7) | (b & 0x7F)
            if not b & 0x80:
                return n

    def add_text(value):
        el = stack[-1]
        if el[2] is None:
            el[2] = value
        elif isinstance(el[2], list):
            if (value.strip() if isinstance(value, str) else value):
                raise ValueError('mixed content')
        elif isinstance(el[2], bytes) or isinstance(value, bytes):
            el[2] = (el[2] if isinstance(el[2], bytes) else el[2].encode()) + \
                (value if isinstance(value, bytes) else value.encode())
        else:
            el[2] += value

    def close(el):
        nonlocal root
        if stack:
            parent = stack[-1]
            if parent[2] is None or (isinstance(parent[2], str) and not parent[2].strip()):
                parent[2] = []
            parent[2].append(el)
        else:
            root = el

    while pos < len(data):
        token = data[pos]
        pos += 1
        if token == 0x00:
            page = data[pos]
            pos += 1
        elif token == 0x01:
            close(stack.pop())
        elif token == 0x03:
            end = data.index(0, pos)
            add_text(data[pos:end].decode())
            pos = end + 1
        elif token == 0xC3:
            n = mb()
            add_text(bytes(data[pos:pos + n]))
            pos += n
        else:
            name = PAGES[page][1][token & 0x3F]
            el = [page, name, None]
            if token & 0x40:
                stack.append(el)
            else:
                close(el)
    return root


def text_of(el):
    c = el[2]
    if c is None:
        return ''
    if isinstance(c, bytes):
        return c.decode(errors='replace')
    if isinstance(c, list):
        return ''
    return c


def matches(want, got, env):
    """Subset match, as replay.rs: `*`, `~part`, `$capture`."""
    if want[0] != got[0] or want[1] != got[1]:
        return False
    w = want[2]
    if w is None:
        return True
    if isinstance(w, list):
        have = got[2] if isinstance(got[2], list) else []
        used = [False] * len(have)
        for child in w:
            for i, cand in enumerate(have):
                if used[i]:
                    continue
                trial = dict(env)
                if matches(child, cand, trial):
                    env.clear()
                    env.update(trial)
                    used[i] = True
                    break
            else:
                return False
        return True
    if isinstance(w, bytes):
        return isinstance(got[2], bytes) and got[2] == w
    want_text, have = w.strip(), text_of(got).strip()
    if want_text == '*':
        return True
    if want_text.startswith('~'):
        return want_text[1:] in have
    if want_text.startswith('$'):
        name = want_text[1:]
        if name in env:
            return env[name] == have
        env[name] = have
        return True
    return want_text == have


def substitute(text, env):
    for name in sorted(env, key=len, reverse=True):
        text = text.replace('$' + name, env[name])
    return text


# ----------------------------------------------------------------- client ----

CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
COMMANDS = {'Sync': 0, 'SendMail': 1, 'SmartForward': 2, 'SmartReply': 3, 'FolderSync': 9,
            'FolderCreate': 10, 'FolderDelete': 11, 'FolderUpdate': 12, 'MoveItems': 13,
            'GetItemEstimate': 14, 'MeetingResponse': 15, 'Search': 16, 'Settings': 17,
            'Ping': 18, 'ItemOperations': 19, 'Provision': 20}


def request(method, url, body=None, headers=None, auth=None, timeout=600):
    req = urllib.request.Request(url, data=body, method=method)
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    if auth:
        req.add_header('Authorization', 'Basic ' + base64.b64encode(f'{auth[0]}:{auth[1]}'.encode()).decode())
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=timeout) as r:
            return r.status, dict(r.headers), r.read()
    except urllib.error.HTTPError as e:
        return e.code, dict(e.headers), e.read()


def base64_query(device, command, params):
    version = 141 if device['version'] == '14.1' else 161
    out = bytearray([version, COMMANDS[command], 0x09, 0x04, len(device['id'])])
    out += device['id'].encode()
    out.append(0)
    out.append(len(device['kind']))
    out += device['kind'].encode()
    for k, v in params:
        tag = {'CollectionId': 1, 'ItemId': 3, 'LongId': 4}.get(k)
        if tag:
            out += bytes([tag, len(v)]) + v.encode()
    return base64.b64encode(bytes(out)).decode()


class Replay:
    def __init__(self, base, account):
        self.base = base.rstrip('/')
        self.account = account
        self.next_received = 0
        self.steps = 0

    # -- helpers the scripts use ------------------------------------------

    def jmap(self, calls):
        _, _, body = request('POST', self.base + '/jmap/', json.dumps({
            'using': ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:mail'],
            'methodCalls': calls}).encode(), {'Content-Type': 'application/json'}, self.account)
        return json.loads(body)['methodResponses']

    def import_message(self, raw):
        _, _, body = request('GET', self.base + '/jmap/session', auth=self.account)
        acct = json.loads(body)['primaryAccounts']['urn:ietf:params:jmap:mail']
        status, _, body = request('POST', f'{self.base}/jmap/upload/{acct}/', raw.encode(),
                                  {'Content-Type': 'message/rfc822'}, self.account)
        blob = json.loads(body)['blobId']
        inbox = self.jmap([['Mailbox/query', {'accountId': acct, 'filter': {'role': 'inbox'}}, 'q']])[0][1]['ids'][0]
        now = max(int(time.time()), self.next_received + 1)
        self.next_received = now
        r = self.jmap([['Email/import', {'accountId': acct, 'emails': {'m': {
            'blobId': blob, 'mailboxIds': {inbox: True},
            'receivedAt': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(now))}}}, 'i']])
        assert r[0][1].get('created', {}).get('m'), r

    def deliver(self, subject):
        self.import_message(
            f'From: Carol Clark <carol@example.org>\r\nTo: {self.account[0]}\r\nSubject: {subject}\r\n'
            f'Message-ID: <{subject.replace(" ", ".")}@example.org>\r\n'
            f'Date: Mon, 23 Nov 2026 10:00:00 +0000\r\n\r\nHello, this is "{subject}".\r\n')

    def deliver_invite(self, subject):
        uid = subject.replace(' ', '-') + '@example.org'
        name = self.account[0]
        self.import_message(
            f'From: Carol Clark <carol@example.org>\r\nTo: {name}\r\nSubject: {subject}\r\n'
            f'Message-ID: <invite-{uid}>\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\n'
            'MIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary="b"\r\n\r\n'
            '--b\r\nContent-Type: text/plain\r\n\r\nYou are invited.\r\n'
            '--b\r\nContent-Type: text/calendar; method=REQUEST; charset=utf-8\r\n\r\n'
            'BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Test//EN\r\nMETHOD:REQUEST\r\n'
            f'BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261123T100000Z\r\n'
            f'DTSTART:20261202T090000Z\r\nDTEND:20261202T100000Z\r\nSUMMARY:{subject}\r\n'
            'ORGANIZER;CN=Carol Clark:mailto:carol@example.org\r\n'
            f'ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:{name}\r\nSEQUENCE:0\r\n'
            'END:VEVENT\r\nEND:VCALENDAR\r\n--b--\r\n')

    def autodiscover(self, dialect):
        email = self.account[0]
        if dialect == 'json':
            return request('GET', f'{self.base}/autodiscover/autodiscover.json/v1.0/{email}?Protocol=ActiveSync')[2]
        if dialect == 'pox':
            body = ('<?xml version="1.0" encoding="utf-8"?><Autodiscover xmlns="http://schemas.microsoft.com/'
                    'exchange/autodiscover/mobilesync/requestschema/2006"><Request><EMailAddress>'
                    f'{email}</EMailAddress><AcceptableResponseSchema>http://schemas.microsoft.com/exchange/'
                    'autodiscover/mobilesync/responseschema/2006</AcceptableResponseSchema></Request>'
                    '</Autodiscover>')
            return request('POST', f'{self.base}/autodiscover/autodiscover.xml', body.encode(),
                           {'Content-Type': 'text/xml'})[2]
        body = ('<?xml version="1.0" encoding="utf-8"?><soap:Envelope xmlns:a="http://schemas.microsoft.com/'
                'exchange/2010/Autodiscover" xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body>'
                f'<a:GetUserSettingsRequestMessage><a:Request><a:Users><a:User><a:Mailbox>{email}'
                '</a:Mailbox></a:User></a:Users><a:RequestedSettings><a:Setting>UserDisplayName</a:Setting>'
                '<a:Setting>ExternalImap4Connections</a:Setting><a:Setting>ExternalEwsUrl</a:Setting>'
                '</a:RequestedSettings></a:Request></a:GetUserSettingsRequestMessage></soap:Body>'
                '</soap:Envelope>')
        return request('POST', f'{self.base}/autodiscover/autodiscover.svc', body.encode(),
                       {'Content-Type': 'text/xml; charset=utf-8'})[2]

    def send(self, device, command, params, headers, xml):
        url = self.base + '/Microsoft-Server-ActiveSync'
        hdrs = {'User-Agent': device['agent']}
        if command == 'OPTIONS':
            method, body = 'OPTIONS', None
        else:
            method = 'POST'
            if device['base64']:
                query = base64_query(device, command, params)
            else:
                query = f"Cmd={command}&User={self.account[0]}&DeviceId={device['id']}&DeviceType={device['kind']}"
                query += ''.join(f'&{k}={v}' for k, v in params)
                hdrs['MS-ASProtocolVersion'] = device['version']
            url += '?' + query
            hdrs['Content-Type'] = 'application/vnd.ms-sync.wbxml'
            body = encode(from_xml(xml)) if xml else None
        hdrs.update(headers)
        return request(method, url, body, hdrs, self.account)

    # -- the script ----------------------------------------------------------

    def run(self, name, script):
        lines = script.splitlines()
        env, device, later = {}, None, None
        i = 0

        def block(stop):
            nonlocal i
            headers, body = [], []
            while i < len(lines):
                t = lines[i].lstrip()
                if t.startswith(stop) or t.startswith('<<<') or t.startswith('!') or t.startswith('@'):
                    break
                i += 1
                if t.startswith('? '):
                    k, v = t[2:].split(':', 1)
                    headers.append((k.strip(), v.strip()))
                elif not t.startswith('#'):
                    body.append(lines[i - 1])
            text = '\n'.join(body).strip()
            return headers, text or None

        def contains_lines():
            nonlocal i
            wanted = []
            while i < len(lines) and lines[i].lstrip().startswith('contains '):
                wanted.append(lines[i].lstrip()[len('contains '):])
                i += 1
            return wanted

        while i < len(lines):
            line = lines[i].rstrip()
            at = f'{name}:{i + 1}'
            i += 1
            if not line.strip() or line.startswith('#'):
                continue
            if line.startswith('@device '):
                p = line.split()[1:]
                device = {'id': p[0], 'kind': p[1], 'agent': p[2], 'version': p[3], 'base64': p[4] == 'base64'}
            elif line.startswith('!deliver-later '):
                secs, subject = line[len('!deliver-later '):].split(' ', 1)
                later = threading.Timer(int(secs), self.deliver, [subject])
                later.start()
            elif line.startswith('!deliver-invite '):
                self.deliver_invite(line[len('!deliver-invite '):])
            elif line.startswith('!deliver '):
                self.deliver(line[len('!deliver '):])
            elif line.startswith('!dav-put '):
                path = line[len('!dav-put '):].strip()
                text = []
                while lines[i].strip() != '!end':
                    text.append(lines[i].strip())
                    i += 1
                i += 1
                text = '\r\n'.join(text) + '\r\n'
                ctype = 'text/vcard; charset=utf-8' if 'BEGIN:VCARD' in text else 'text/calendar; charset=utf-8'
                status, _, _ = request('PUT', os.environ['DAV_URL'].rstrip('/') + '/' + path, text.encode(),
                                       {'Content-Type': ctype}, self.account)
                assert 200 <= status < 300, f'{at}: PUT {path}: {status}'
            elif line.startswith('!setup'):
                path = line[len('!setup'):].strip()
                wanted = contains_lines()
                status, headers, body = request('GET', f'{self.base}/setup{path}',
                                                auth=self.account if path else None)
                text = body.decode(errors='replace')
                ctype = {k.lower(): v for k, v in headers.items()}.get('content-type', '')
                for w in wanted:
                    assert w in text or w in ctype, f'{at}: /setup{path} lacks {w!r}:\n{text[:800]}'
            elif line.startswith('!autodiscover '):
                dialect = line.split()[1]
                wanted = contains_lines()
                text = self.autodiscover(dialect).decode(errors='replace')
                for w in wanted:
                    assert w in text, f'{at}: autodiscover {dialect} lacks {w!r}:\n{text[:800]}'
            elif line.startswith('>>> '):
                words = line[4:].split()
                command = words[0]
                params = [(k, substitute(v, env)) for k, v in (w.split('=', 1) for w in words[1:])]
                headers, body = block('<<<')
                headers = {k: substitute(v, env) for k, v in headers}
                expect = lines[i][4:].split()
                i += 1
                status_want = int(expect[0])
                empty = len(expect) > 1 and expect[1] == 'empty'
                exp_headers, exp_body = block('>>> ')
                t0 = time.perf_counter()
                status, got_headers, data = self.send(device, command, params, headers,
                                                      substitute(body, env) if body else None)
                ms = (time.perf_counter() - t0) * 1000
                if later:
                    later.join()
                    later = None
                assert status == status_want, f'{at}: {command} answered {status}: {data[:300]!r}'
                lower = {k.lower(): v for k, v in got_headers.items()}
                for k, v in exp_headers:
                    assert v in lower.get(k.lower(), ''), f'{at}: header {k} is {lower.get(k.lower())!r}, wanted {v!r}'
                if empty:
                    assert not data, f'{at}: wanted an empty answer, got {len(data)} bytes'
                if exp_body:
                    want = from_xml(exp_body)
                    got = decode(data)
                    trial = dict(env)
                    assert matches(want, got, trial), \
                        f'{at}: {command} answer does not match.\nExpected:\n{to_xml(want)}\nGot:\n{to_xml(got)}'
                    env = trial
                self.steps += 1
                print(f'  {at}: {command} ok ({ms:.0f} ms)', flush=True)
            else:
                raise AssertionError(f'{at}: cannot read {line!r}')


def main():
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    base = sys.argv[1]
    first = tuple(sys.argv[2].split(':', 1))
    second = tuple(sys.argv[3].split(':', 1))
    files = sys.argv[4:] or ['iphone.txt', 'android.txt', 'outlook.txt'] + \
        (['nextcloud.txt'] if os.environ.get('DAV_URL') else [])
    failed = 0
    for name in files:
        account = second if name == 'outlook.txt' else first
        replay = Replay(base, account)
        print(f'Replaying {name} as {account[0]}...', flush=True)
        try:
            replay.run(name, open(os.path.join(RESOURCES, name)).read())
            print(f'PASS  {name}: {replay.steps} requests', flush=True)
        except AssertionError as err:
            failed += 1
            print(f'FAIL  {name} after {replay.steps} requests: {err}', flush=True)
    sys.exit(1 if failed else 0)


if __name__ == '__main__':
    main()
