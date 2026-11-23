#!/usr/bin/env python3
"""Remove calendars, contacts, files and WebDAV from the console schema.

The admin console builds its pages from resources/schema/schema.json.gz,
which the server serves at /api/schema. Hearth Mail removed those features
from the Rust code (docs/decisions.md, "Calendars, contacts and files
removed"); this takes them out of the schema the same way, so the console
shows no page, field, choice or permission for them.

What goes:
  * the objects, their schemas, fields, forms and lists (OBJECTS);
  * union variants and layout links that point at them;
  * enum values whose names the generated Rust parsers no longer know,
    found by comparing the string literals of the registry and trc parsers
    at BASE with the working tree (so the list can't drift from the code);
  * fields of settings that stay but are no longer read (HIDDEN_FIELDS).
    Their slots stay in the stored objects (pickled by position), only the
    console stops showing them.

Usage: tools/hearth/strip-schema.py [--base REV] [--check]
  --base   the commit the features were removed from (default: the
           recorded upstream base, v27.1-rc.39)
  --check  exit 1 if the schema still has anything to remove (CI)
The checksum file next to the schema is rewritten with it.
"""
import argparse
import base64
import gzip
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCHEMA = ROOT / 'resources/schema/schema.json.gz'
SCHEMA_HASH = ROOT / 'resources/schema/schema.json.sha256'
PARSERS = ['crates/registry/src/schema/enums_impl.rs', 'crates/trc/src/event/enums_impl.rs',
           'crates/registry/src/schema/properties_impl.rs']

# Objects and schemas that are gone whole, with every view of them.
OBJECTS = {
    'AddressBook', 'Calendar', 'AddressBookRights', 'CalendarRights',
    'x:AddressBook', 'x:Calendar', 'x:CalendarAlarm', 'x:CalendarScheduling', 'x:FileStorage',
    'x:WebDav', 'x:ArchivedCalendarEvent', 'x:ArchivedContactCard', 'x:ArchivedFileNode',
    'x:TaskCalendarAlarmEmail', 'x:TaskCalendarAlarmNotification', 'x:TaskCalendarItipContents',
    'x:TaskCalendarItipMessage', 'VCardVersion', 'SearchCalendarField', 'SearchContactField',
    'SearchFileField',
}

# Fields of kept settings that nothing reads any more.
HIDDEN_FIELDS = {
    'x:Cache': {'contacts', 'events', 'scheduling', 'files'},
    'x:Jmap': {'parseLimitEvent', 'parseLimitContact'},
    'x:Search': {'indexCalendar', 'indexCalendarFields', 'indexContacts', 'indexContactFields'},
    'x:DataRetention': {'expungeSchedulingInboxAfter'},
    'x:SpamSettings': {'trustContacts'},
    'x:SpamClassifier': {'learnHamFromCard'},
}


def literals(text):
    return set(re.findall(r'"((?:[^"\\]|\\.)*)"', text))


def removed_names(base):
    """String literals the generated parsers had at `base` and no longer have."""
    gone = set()
    for path in PARSERS:
        old = subprocess.check_output(['git', 'show', f'{base}:{path}'], cwd=ROOT, text=True)
        new = (ROOT / path).read_text()
        gone |= literals(old) - literals(new)
    return gone


def refers_to_removed(node):
    """A schema node pointing at a removed object or view."""
    if not isinstance(node, dict):
        return False
    for key in ('schemaName', 'viewName', 'objectName', 'object'):
        value = node.get(key)
        if isinstance(value, str) and value.split('/')[0] in OBJECTS:
            return True
    return False


def prune(node, gone_enum_names):
    if isinstance(node, dict):
        if 'link' in node and refers_to_removed(node['link']):
            return None
        if 'container' in node:
            inner = prune(node['container'], gone_enum_names)
            if inner is None or not inner.get('items'):
                return None
            node['container'] = inner
            return node
        if refers_to_removed(node):
            return None
        for key in list(node):
            value = prune(node[key], gone_enum_names)
            if value is None:
                del node[key]
            else:
                node[key] = value
        return node
    if isinstance(node, list):
        out = []
        for item in node:
            value = prune(item, gone_enum_names)
            if value is not None:
                out.append(value)
        return out
    return node


def strip(schema, gone):
    for section in ('objects', 'schemas', 'fields', 'forms', 'lists', 'enums'):
        for name in list(schema.get(section, {})):
            if name.split('/')[0] in OBJECTS:
                del schema[section][name]
    # Enum values the code no longer parses
    for name, values in schema['enums'].items():
        if isinstance(values, list):
            schema['enums'][name] = [v for v in values
                                     if not (isinstance(v, dict) and v.get('name') in gone)]
    # Fields of kept settings nothing reads
    for obj, props in HIDDEN_FIELDS.items():
        fields = schema['fields'].get(obj, {})
        for prop in props:
            fields.get('properties', {}).pop(prop, None)
            fields.get('defaults', {}).pop(prop, None)
        form = schema['forms'].get(obj)
        if form:
            for section in form.get('sections', []):
                section['fields'] = [f for f in section.get('fields', [])
                                     if f.get('name') not in props]
            form['sections'] = [s for s in form.get('sections', []) if s.get('fields')]
    # Defaults keyed by a removed enum value (the CalDAV, CardDAV and WebDAV
    # service entries of x:SystemSettings)
    for fields in schema['fields'].values():
        for value in fields.get('defaults', {}).values():
            if isinstance(value, dict):
                for key in [k for k in value if k in gone]:
                    del value[key]
    for section in ('schemas', 'forms', 'lists', 'layouts', 'dashboards'):
        schema[section] = prune(schema[section], gone)
    # The settings group that held file storage keeps only sharing
    for layout in schema['layouts']:
        for item in layout.get('items', []):
            container = item.get('container') if isinstance(item, dict) else None
            if container and container.get('name') == 'Files & Sharing':
                container['name'] = 'Sharing'
    return schema


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--base', default='v27.1-rc.39')
    ap.add_argument('--check', action='store_true')
    args = ap.parse_args()
    raw = gzip.decompress(SCHEMA.read_bytes())
    schema = json.loads(raw)
    before = json.dumps(schema, sort_keys=True)
    gone = removed_names(args.base)
    schema = strip(schema, gone)
    after = json.dumps(schema, sort_keys=True)
    if args.check:
        if before != after:
            print('resources/schema/schema.json.gz still has removed features; '
                  'run tools/hearth/strip-schema.py', file=sys.stderr)
            sys.exit(1)
        return
    # Same encoding as tools/fork/expr-schema.py, so the two agree on the
    # bytes and on the checksum the console caches the schema by.
    text = json.dumps(schema, ensure_ascii=False, separators=(',', ':'))
    out = gzip.compress(text.encode('utf-8'), compresslevel=9, mtime=0)
    digest = base64.urlsafe_b64encode(hashlib.sha256(out).digest()).decode().rstrip('=')
    SCHEMA.write_bytes(out)
    SCHEMA_HASH.write_text(digest)
    print(f'schema: {len(before)} -> {len(after)} bytes; {len(gone)} names gone from the parsers')


if __name__ == '__main__':
    main()
