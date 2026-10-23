#!/usr/bin/env python3
"""Build a local SWF with verified AIR extension class definitions preloaded."""
import argparse
import json
from pathlib import Path
import re
import struct
import subprocess
import tempfile
import zipfile
import zlib

from inspect_apk import RUNTIME_ASSETS, decode_swf, inspect, sha256, swf_tags

BASE = 'com.ironsource.adobeair'
ADAPTERS = {BASE + '.' + name for name in
            ('adcolony', 'admob', 'applovin', 'chartboost', 'unityads')}
SEGMENTS = {
    'c74069e86da3e2e202372a4119aca2565722b4ae094682c807a44323a89e6ece',
    '206b5680e5946869b2494d9ba8d8d459f12fcbb865e535737168dd706a9b62e1',
}


def abc_tags(data):
    body, offset = decode_swf(data)
    return [payload for tag, payload in swf_tags(body, offset) if tag == 82]


def abc_bytes(payload):
    if len(payload) < 5 or struct.unpack_from('<I', payload)[0] != 1:
        raise ValueError('Expected a lazy DoABC tag')
    return payload[payload.index(0, 4) + 1:]


def encode_tag(tag, payload):
    return struct.pack('<HI', (tag << 6) | 63, len(payload)) + payload


GAME_SETTINGS_PATCH = {
    'original_abc_sha256': '2dbf86946513c528bce4bd81991ce7a58cfe203bf912d9952d36076fd98adc61',
    'patched_abc_sha256': 'a920311b423fc8374c6de7b112e73a97e825fbacd713d3e05658dc9a9b5630e1',
    'initializer_method': 40, 'premium': True, 'playAds': False,
}


def patch_game_settings(payload):
    bytecode = abc_bytes(payload)
    if sha256(bytecode) != GAME_SETTINGS_PATCH['original_abc_sha256']:
        raise ValueError('Unexpected game bytecode for settings patch')
    # This locked APK has one terminal return in globalVar.cinit. Its fresh-profile
    # branch jumps to that return, so keep every original branch byte unchanged.
    # The receiver is the class; the two public QName indices are 5001 and 5004.
    insertion = bytes.fromhex('d026618927d027618c27')
    patched = (bytecode[:464493] + bytes.fromhex('f533') + bytecode[464495:471129]
               + insertion + bytecode[471129:])
    if sha256(patched) != GAME_SETTINGS_PATCH['patched_abc_sha256']:
        raise ValueError('Settings patch differs from its verified bytecode')
    return payload[:-len(bytecode)] + patched


def runtime_support(source=None):
    root = Path(__file__).resolve().parents[1]
    compiler_info = subprocess.run(['rustc', '-vV'], cwd=root, check=True,
                                   capture_output=True, text=True, timeout=30).stdout
    host = next(line.split(': ', 1)[1] for line in compiler_info.splitlines()
                if line.startswith('host: '))
    metadata = json.loads(subprocess.run(
        ['cargo', 'metadata', '--format-version', '1', '--locked', '--offline', '--filter-platform', host],
        cwd=root, check=True, capture_output=True, text=True, timeout=30).stdout)
    core = next(package for package in metadata['packages'] if package['name'] == 'ruffle_core')
    sdk = Path(core['manifest_path']).parent.parent
    compiler = sdk / 'core/build_playerglobal/asc.jar'
    target = Path(metadata['target_directory'])
    candidates = [path for profile in ('debug', 'release')
                  for path in (target / profile / 'build').glob('ruffle_core*/**/playerglobal.swf')]
    if not candidates:
        raise ValueError('Build the host runtime first: cargo build --locked (use the same CARGO_TARGET_DIR)')
    playerglobal = max(candidates, key=lambda path: path.stat().st_mtime).read_bytes()
    definitions = abc_tags(playerglobal)
    if len(definitions) != 1:
        raise ValueError('Unexpected playerglobal ABC inventory')
    destination = root / 'build/runtime'
    destination.mkdir(parents=True, exist_ok=True)
    source = source or root / 'runtime/flash/events/PermissionEvent.as'
    with tempfile.TemporaryDirectory(dir=destination) as temporary:
        temporary = Path(temporary)
        imported = temporary / 'playerglobal.abc'
        imported.write_bytes(abc_bytes(definitions[0]))
        result = subprocess.run(
            ['java', '-classpath', str(compiler), 'macromedia.asc.embedding.ScriptCompiler',
             '-AS3', '-strict', '-optimize', '-import', str(imported),
             '-outdir', str(temporary), '-out', 'lonewolf-platform', str(source)],
            cwd=root, capture_output=True, text=True, timeout=30)
        generated = temporary / 'lonewolf-platform.abc'
        diagnostics = result.stdout + result.stderr
        if result.returncode != 0 or not generated.is_file() or re.search(r'\bError\b', diagnostics):
            raise ValueError(f'Runtime support compilation failed: {diagnostics}')
        bytecode = generated.read_bytes()
    if len(bytecode) < 4 or bytecode[:4] != struct.pack('<HH', 16, 46):
        raise ValueError('Runtime support compiler did not produce a valid ABC header')
    (destination / 'lonewolf-platform.abc').write_bytes(bytecode)
    return bytecode, {'source_sha256': sha256(source.read_bytes()),
                      'compiler_sha256': sha256(compiler.read_bytes()),
                      'playerglobal_sha256': sha256(playerglobal), 'abc_sha256': sha256(bytecode)}


def extract_runtime_assets(apk_path, inventory, directory):
    entries = {entry['path']: entry for entry in inventory['inputs']}
    resources = []
    with zipfile.ZipFile(apk_path) as apk:
        for name in RUNTIME_ASSETS:
            data = apk.read('assets/' + name)
            entry = entries['assets/' + name]
            if len(data) != entry['bytes'] or sha256(data) != entry['sha256']:
                raise ValueError(f'Runtime asset differs from inventory: {name}')
            destination = directory / name
            if destination.is_symlink():
                raise ValueError(f'Runtime asset output must not be a symlink: {name}')
            resources.append((destination, data))
    directory.mkdir(parents=True, exist_ok=True)
    for destination, data in resources:
        destination.write_bytes(data)
    return [path for path, _ in resources]


def prepare(apk_path, lock_path, output):
    if output.suffix.lower() != '.swf':
        raise ValueError('Runtime output must have a .swf extension')
    protected = {apk_path.resolve(), lock_path.resolve()}
    if output.resolve() in protected or output.with_suffix('.json').resolve() in protected:
        raise ValueError('Runtime output would overwrite an input')
    inventory = json.loads(lock_path.read_text())
    if inspect(apk_path) != inventory:
        raise ValueError('APK does not match the recorded inputs')
    with zipfile.ZipFile(apk_path) as apk:
        source = apk.read('assets/' + inventory['air']['content'])
        libraries = {
            extension: abc_tags(apk.read(
                f'assets/META-INF/AIR/extensions/{extension}/library.swf'))
            for extension in inventory['air']['extensions']
        }
    base = libraries[BASE]
    if len(base) != 3 or sha256(abc_bytes(base[1])) not in SEGMENTS:
        raise ValueError('Unexpected IronSource base definitions')
    for extension in sorted(ADAPTERS):
        adapter = libraries[extension]
        if (len(adapter) != 3 or abc_bytes(adapter[0]) != abc_bytes(base[0])
                or abc_bytes(adapter[2]) != abc_bytes(base[2])
                or sha256(abc_bytes(adapter[1])) not in SEGMENTS):
            raise ValueError(f'Unexpected duplicate definitions: {extension}')
    selected = []
    seen = set()
    for extension in [BASE] + sorted(set(libraries) - ADAPTERS - {BASE}):
        for payload in libraries[extension]:
            digest = sha256(abc_bytes(payload))
            if digest in seen:
                raise ValueError(f'Unexpected duplicate ABC: {extension}')
            seen.add(digest)
            selected.append((extension, payload))
    if len(selected) != 88:
        raise ValueError('Unexpected extension ABC inventory')
    support, support_report = runtime_support()
    body, offset = decode_swf(source)
    tags = list(swf_tags(body, offset))
    first_abc = next(i for i, (tag, _) in enumerate(tags) if tag == 82)
    if sum(tag == 82 for tag, _ in tags) != 1:
        raise ValueError('Unexpected original game ABC inventory')
    tags[first_abc] = (82, patch_game_settings(tags[first_abc][1]))
    support_tag = struct.pack('<I', 1) + b'lonewolf-platform\0' + support
    combined = tags[:first_abc] + [(82, support_tag)] + [(82, data) for _, data in selected] + tags[first_abc:]
    result_body = body[:offset] + b''.join(encode_tag(tag, data) for tag, data in combined)
    result = b'CWS' + source[3:4] + struct.pack('<I', len(result_body) + 8) + zlib.compress(result_body)
    output.parent.mkdir(parents=True, exist_ok=True)
    extract_runtime_assets(apk_path, inventory, output.parent)
    output.write_bytes(result)
    report = {'source_sha256': sha256(source), 'output_sha256': sha256(result),
              'extension_abc_count': len(selected), 'excluded_adapters': sorted(ADAPTERS),
              'runtime_abc_count': 1, 'runtime_support': support_report,
              'game_settings_patch': GAME_SETTINGS_PATCH,
              'extensions': [{'id': name, 'abc_sha256': sha256(abc_bytes(data))}
                             for name, data in selected],
              'note': 'AIR support defines classes only; Android native services are not implemented.'}
    output.with_suffix('.json').write_text(json.dumps(report, indent=2) + '\n')
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('apk', type=Path)
    parser.add_argument('--lock', type=Path, default=Path(__file__).with_name('inputs.lock.json'))
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    report = prepare(args.apk, args.lock, args.output)
    print(f"Prepared {args.output}: {report['extension_abc_count']} extension ABC blocks")


if __name__ == '__main__':
    main()
