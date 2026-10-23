#!/usr/bin/env python3
"""Inspect local AIR port inputs without extracting or executing game code."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import struct
import sys
import xml.etree.ElementTree as ET
import zipfile
import zlib


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def decode_swf(data):
    if len(data) < 9:
        raise ValueError('Truncated SWF header')
    signature = data[:3]
    if signature not in (b'CWS', b'FWS'):
        raise ValueError('Expected zlib-compressed or uncompressed SWF')
    declared = struct.unpack_from('<I', data, 4)[0]
    body = zlib.decompress(data[8:]) if signature == b'CWS' else data[8:]
    if len(body) + 8 != declared:
        raise ValueError('SWF length does not match its header')
    rect_bits = body[0] >> 3
    offset = (5 + 4 * rect_bits + 7) // 8 + 4
    if not rect_bits or offset > len(body):
        raise ValueError('Truncated SWF frame header')
    return body, offset


def swf_tags(body, offset):
    while offset < len(body):
        if offset + 2 > len(body):
            raise ValueError('Truncated SWF tag header')
        header = struct.unpack_from('<H', body, offset)[0]
        offset += 2
        tag, length = header >> 6, header & 63
        if length == 63:
            if offset + 4 > len(body):
                raise ValueError('Truncated SWF long tag header')
            length = struct.unpack_from('<I', body, offset)[0]
            offset += 4
        end = offset + length
        if end > len(body):
            raise ValueError('Truncated SWF tag')
        yield tag, body[offset:end]
        offset = end
        if tag == 0:
            if length or offset != len(body):
                raise ValueError('Invalid SWF end tag or trailing data')
            return
    raise ValueError('Missing SWF end tag')


def swf_info(data):
    body, tags_offset = decode_swf(data)
    bits = ''.join(f'{byte:08b}' for byte in body[:17])
    width = int(bits[:5], 2)
    coordinates = []
    for i in range(4):
        value = int(bits[5 + i * width:5 + (i + 1) * width], 2)
        if value & (1 << (width - 1)):
            value -= 1 << width
        coordinates.append(value)
    offset = (5 + 4 * width + 7) // 8
    rate, frames = struct.unpack_from('<HH', body, offset)
    offset += 4
    tags = Counter()
    as3 = False
    document_class = None
    abc_bytes = 0
    for tag, payload in swf_tags(body, tags_offset):
        tags[tag] += 1
        if tag == 69:
            as3 = bool(struct.unpack_from('<I', payload)[0] & 8)
        elif tag == 76:
            count = struct.unpack_from('<H', payload)[0]
            cursor = 2
            for _ in range(count):
                symbol = struct.unpack_from('<H', payload, cursor)[0]
                cursor += 2
                terminator = payload.index(0, cursor)
                name = payload[cursor:terminator].decode('utf-8')
                if symbol == 0:
                    document_class = name
                cursor = terminator + 1
        elif tag == 82:
            abc_bytes += len(payload)
    return {
        'signature': data[:3].decode(), 'version': data[3],
        'uncompressed_bytes': len(body) + 8, 'actionscript3': as3,
        'width': (coordinates[1] - coordinates[0]) / 20,
        'height': (coordinates[3] - coordinates[2]) / 20,
        'fps': rate / 256, 'frames': frames, 'document_class': document_class,
        'doabc_tag_bytes': abc_bytes,
        'tag_counts': {str(tag): count for tag, count in sorted(tags.items())},
    }


RUNTIME_ASSETS = tuple(f"text_{locale}.xml" for locale in ("en", "ru", "ch", "fr", "gr", "it", "sp", "tk"))


def inspect(path):
    data = path.read_bytes()
    with zipfile.ZipFile(path) as apk:
        names = apk.namelist()
        if len(names) != len(set(names)):
            raise ValueError('APK contains duplicate ZIP entry names')
        application_path = 'assets/META-INF/AIR/application.xml'
        application = ET.fromstring(apk.read(application_path))
        if not application.tag.startswith('{http://ns.adobe.com/air/application/'):
            raise ValueError('Expected an Adobe AIR application descriptor')
        namespace = application.tag.split('}')[0] + '}'

        def field(name):
            return application.findtext(namespace + name)

        initial = application.find(namespace + 'initialWindow')
        if initial is None:
            raise ValueError('AIR descriptor is missing initialWindow')
        content = initial.findtext(namespace + 'content')
        if not content:
            raise ValueError('AIR descriptor is missing its SWF content')
        swf_path = 'assets/' + content
        extensions = application.find(namespace + 'extensions')
        entries = []
        selected = [application_path, swf_path] + ["assets/" + name for name in RUNTIME_ASSETS]
        selected += sorted(name for name in names
                           if (name.startswith('lib/') and name.endswith('.so'))
                           or (name.startswith('classes') and name.endswith('.dex')))
        for name in selected:
            payload = apk.read(name)
            entry = {'path': name, 'bytes': len(payload), 'sha256': sha256(payload)}
            if name.endswith('.so'):
                if payload[:6] != b'\x7fELF\x01\x01':
                    raise ValueError(f'Expected little-endian ELF32: {name}')
                entry['elf_machine'] = struct.unpack_from('<H', payload, 18)[0]
                entry['elf_flags'] = struct.unpack_from('<I', payload, 36)[0]
            elif name == swf_path:
                entry['swf'] = swf_info(payload)
            entries.append(entry)
        return {
            'schema_version': 1,
            'apk': {'bytes': len(data), 'sha256': sha256(data), 'entries': len(names)},
            'air': {
                'namespace': namespace[1:-1], 'id': field('id'),
                'version': field('versionNumber'), 'content': content,
                'render_mode': initial.findtext(namespace + 'renderMode'),
                'extensions': sorted(element.text for element in extensions)
                              if extensions is not None else [],
            },
            'inputs': entries,
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('apk', nargs='?', type=Path,
                        default=Path(__file__).resolve().parents[1] / 'LONEWOLF_v1.2.90_Mod.apk')
    parser.add_argument('--verify', type=Path, help='Compare against a recorded inventory')
    parser.add_argument('--output', type=Path, help='Write inventory JSON instead of stdout')
    args = parser.parse_args()
    try:
        report = inspect(args.apk)
        if args.verify and report != json.loads(args.verify.read_text()):
            raise ValueError('APK inventory differs from the recorded inputs')
        encoded = json.dumps(report, indent=2, ensure_ascii=False) + '\n'
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(encoded)
            print(f'Inventory written to {args.output}')
        elif args.verify:
            print(f'PASS: APK, AIR metadata, SWF and native inputs match {args.verify}')
        else:
            print(encoded, end='')
    except (OSError, ValueError, KeyError, ET.ParseError, zipfile.BadZipFile,
            struct.error, zlib.error) as error:
        sys.exit(f'Input inspection failed: {error}')


if __name__ == '__main__':
    main()
