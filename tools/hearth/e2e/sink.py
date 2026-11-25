#!/usr/bin/env python3
"""A remote mail server for the end-to-end run: the MX of remote.example.net.

Hearth Mail's outbound queue delivers to it as it would to any mail
server on the internet (an MX lookup through the test resolver, then SMTP
on port 25), so the run can see what leaves the server: the envelope,
whether STARTTLS was used, and the message as sent (with its DKIM
signature). Each message is written to /out/<n>.eml with a small JSON
beside it (/out/<n>.json: client address, EHLO name, MAIL FROM, RCPT TO,
TLS).

Standard library only, so it runs in the plain python image. It speaks
just enough SMTP for a sending server: EHLO/HELO, STARTTLS (with the
certificate in /certs), MAIL, RCPT, DATA, RSET, NOOP, QUIT. An RCPT to
an address starting with "reject" is refused with 550, so the run can
check bounces.
"""
import asyncio
import json
import os
import ssl

OUT = '/out'
counter = 0


def tls_context():
    ctx = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
    ctx.load_cert_chain('/certs/sink.crt', '/certs/sink.key')
    return ctx


async def session(reader, writer):
    global counter
    peer = writer.get_extra_info('peername')[0]
    state = {'tls': False, 'ehlo': '', 'from': '', 'to': []}

    async def say(line):
        writer.write(line.encode() + b'\r\n')
        await writer.drain()

    await say('220 sink.remote.example.net ESMTP test sink')
    while True:
        raw = await reader.readline()
        if not raw:
            break
        line = raw.decode(errors='replace').rstrip('\r\n')
        verb = line.split(' ', 1)[0].upper()
        if verb in ('EHLO', 'HELO'):
            state['ehlo'] = line[5:].strip()
            if verb == 'HELO':
                await say('250 sink.remote.example.net')
                continue
            await say('250-sink.remote.example.net')
            await say('250-SIZE 104857600')
            await say('250-8BITMIME')
            if not state['tls']:
                await say('250-STARTTLS')
            await say('250 ENHANCEDSTATUSCODES')
        elif verb == 'STARTTLS' and not state['tls']:
            await say('220 2.0.0 go ahead')
            await writer.start_tls(tls_context())
            state.update(tls=True, ehlo='', to=[])
        elif verb == 'MAIL':
            state['from'] = line.split(':', 1)[1].split()[0].strip('<>') if ':' in line else ''
            state['to'] = []
            await say('250 2.1.0 ok')
        elif verb == 'RCPT':
            rcpt = line.split(':', 1)[1].split()[0].strip('<>')
            if rcpt.lower().startswith('reject'):
                await say('550 5.1.1 no such user here')
            else:
                state['to'].append(rcpt)
                await say('250 2.1.5 ok')
        elif verb == 'DATA':
            await say('354 end with .')
            lines = []
            while True:
                data = await reader.readline()
                if data in (b'.\r\n', b'.\n', b''):
                    break
                if data.startswith(b'..'):
                    data = data[1:]
                lines.append(data)
            counter += 1
            name = f'{os.getpid()}-{counter:05d}'
            with open(f'{OUT}/{name}.eml', 'wb') as f:
                f.write(b''.join(lines))
            with open(f'{OUT}/{name}.json', 'w') as f:
                json.dump({'peer': peer, 'ehlo': state['ehlo'], 'from': state['from'],
                           'to': state['to'], 'tls': state['tls']}, f)
            await say('250 2.0.0 queued as ' + name)
        elif verb == 'RSET':
            state.update({'from': '', 'to': []})
            await say('250 2.0.0 ok')
        elif verb == 'NOOP':
            await say('250 2.0.0 ok')
        elif verb == 'QUIT':
            await say('221 2.0.0 bye')
            break
        else:
            await say('502 5.5.2 not here')
    writer.close()


async def main():
    server = await asyncio.start_server(session, '0.0.0.0', 25)
    async with server:
        await server.serve_forever()


if __name__ == '__main__':
    asyncio.run(main())
