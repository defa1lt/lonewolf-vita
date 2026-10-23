#!/usr/bin/env python3
"""Build a private Vita test VPK from verified local game and approved artwork."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = 'armv7-sony-vita-newlibeabihf'
TITLE_ID = 'LONE00001'

from prepare_runtime import GAME_SETTINGS_PATCH, extract_runtime_assets


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def build(sdk, release=True):
    sdk = sdk.resolve()
    environment = dict(os.environ, VITASDK=str(sdk))
    environment['PATH'] = str(sdk / 'bin') + os.pathsep + environment.get('PATH', '')
    for kind, binary in [('CC', 'gcc'), ('AR', 'ar'), ('RANLIB', 'ranlib')]:
        environment[kind + '_' + TARGET.replace('-', '_')] = str(sdk / f'bin/arm-vita-eabi-{binary}')
    game = ROOT / 'build/Lonewolf-air.swf'
    prepared = json.loads(game.with_suffix('.json').read_text())
    if prepared.get('game_settings_patch') != GAME_SETTINGS_PATCH:
        raise ValueError('Re-run prepare_runtime.py to apply current game settings')
    inputs = json.loads((ROOT / 'tools/inputs.lock.json').read_text())
    original = next(entry for entry in inputs['inputs'] if entry['path'] == 'assets/Lonewolf.swf')
    if digest(game) != prepared['output_sha256'] or prepared['source_sha256'] != original['sha256']:
        raise ValueError('Prepared game does not match its verified inventory')
    if digest(ROOT / 'LONEWOLF_v1.2.90_Mod.apk') != inputs['apk']['sha256']:
        raise ValueError('Source APK differs from the recorded inputs')
    if digest(ROOT / 'runtime/flash/events/PermissionEvent.as') != prepared['runtime_support']['source_sha256']:
        raise ValueError('Re-run prepare_runtime.py after changing AIR support')
    resources = extract_runtime_assets(ROOT / 'LONEWOLF_v1.2.90_Mod.apk', inputs, game.parent)
    artwork = ROOT / 'assets/livearea/sce_sys'
    approved = json.loads((ROOT / 'tools/livearea.lock.json').read_text())
    if approved['approval']['status'] != 'approved':
        raise ValueError('LiveArea exports require the owner’s approval')
    for role, name in [('icon', 'icon0.png'), ('background', 'livearea/contents/bg0.png'),
                       ('startup', 'livearea/contents/startup.png')]:
        image = artwork / name
        data = image.read_bytes()
        if digest(image) != approved[role]['output_sha256']:
            raise ValueError(f'LiveArea image changed after approval: {role}')
        if (data[:8] != b'\x89PNG\r\n\x1a\n' or data[24:26] != b'\x08\x03'
                or list(struct.unpack_from('>II', data, 16)) != approved[role]['output_size']):
            raise ValueError(f'Invalid indexed Vita PNG: {role}')
    manual = sorted((ROOT / 'manual').glob('*.png'))
    if not manual or [p.name for p in manual] != [f'{i:03}.png' for i in range(1, len(manual) + 1)]:
        raise ValueError('Manual pages must start at 001.png without gaps')
    for page in manual:
        data = page.read_bytes()
        if (data[:8] != b'\x89PNG\r\n\x1a\n' or data[24:26] != b'\x08\x03'
                or struct.unpack_from('>II', data, 16) != (960, 544) or data[28] != 0):
            raise ValueError(f'Invalid manual page: {page}')
    command = ['cargo', 'build', '-Zbuild-std=std,panic_abort', '--target', TARGET,
               '--locked', '--message-format=json-render-diagnostics']
    if release:
        command.append('--release')
    result = subprocess.run(command, cwd=ROOT, env=environment, check=True,
                            stdout=subprocess.PIPE, text=True, timeout=2400)
    artifacts = [json.loads(line) for line in result.stdout.splitlines()]
    executables = [artifact['executable'] for artifact in artifacts
                   if artifact.get('reason') == 'compiler-artifact'
                   and artifact.get('target', {}).get('name') == 'lonewolf-vita'
                   and artifact.get('executable')]
    if len(executables) != 1:
        raise ValueError('Cargo did not report exactly one Vita executable')
    profile = 'release' if release else 'debug'
    compiled = Path(executables[0])
    directory = ROOT / 'build/vita'
    directory.mkdir(parents=True, exist_ok=True)
    elf, velf, eboot = [directory / name for name in ('lonewolf.elf', 'lonewolf.velf', 'eboot.bin')]
    subprocess.run([str(sdk / 'bin/arm-vita-eabi-strip'), '--strip-debug', '-o', str(elf), str(compiled)], check=True, timeout=30)
    data = elf.read_bytes()
    if data[:6] != b'\x7fELF\x01\x01' or struct.unpack_from('<H', data, 18)[0] != 40:
        raise ValueError('Expected an ARM ELF32 executable')
    for program, args in [
            ('vita-elf-create', [str(elf), str(velf)]),
            ('vita-make-fself', ['-s', '-c', str(velf), str(eboot)]),
            ('vita-mksfoex', ['-s', 'TITLE_ID=' + TITLE_ID, '-s', 'APP_VER=00.02',
                              '-d', 'ATTRIBUTE2=12', 'LONEWOLF Vita', str(directory / 'param.sfo')])]:
        subprocess.run([str(sdk / 'bin' / program)] + args, check=True, timeout=60)
    if eboot.read_bytes()[:4] != b'SCE\0':
        raise ValueError('Vita signer did not produce a SELF executable')
    vpk = directory / 'LonewolfVita.vpk'
    with zipfile.ZipFile(vpk, 'w', compression=zipfile.ZIP_DEFLATED) as package:
        package.write(eboot, 'eboot.bin')
        package.write(directory / 'param.sfo', 'sce_sys/param.sfo')
        package.write(game, 'game.swf')
        for resource in resources:
            package.write(resource, resource.name)
        for asset in sorted(artwork.rglob('*')):
            if asset.is_file():
                package.write(asset, 'sce_sys/' + asset.relative_to(artwork).as_posix())
        for page in manual:
            package.write(page, 'sce_sys/manual/' + page.name)
        package.write(ROOT / 'vendor/ruffle_render_glow/LICENSE.md', 'licenses/ruffle.txt')
    with zipfile.ZipFile(vpk) as package:
        if package.testzip() is not None:
            raise ValueError('VPK CRC validation failed')
    revision = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, check=True,
                              capture_output=True, text=True, timeout=60).stdout.strip()
    dirty = subprocess.run(['git', 'status', '--porcelain'], cwd=ROOT, check=True,
                           capture_output=True, text=True, timeout=60).stdout != ''
    report = {'title_id': TITLE_ID, 'source_commit': revision, 'source_dirty': dirty,
              'profile': profile, 'elf_sha256': digest(elf), 'eboot_sha256': digest(eboot),
              'game_sha256': digest(game), 'manual': {page.name: digest(page) for page in manual}, 'resources': {path.name: digest(path) for path in resources}, 'vpk_sha256': digest(vpk), 'vpk_bytes': vpk.stat().st_size,
              'hardware_verified': False, 'limitations': ['Audio, gameplay, performance and saves await device verification']}
    (directory / 'package.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f'Vita test candidate: {vpk}\nSHA256: {report["vpk_sha256"]}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--sdk', type=Path, default=os.environ.get('VITASDK'))
    parser.add_argument('--debug', action='store_true', help='Build a slower development candidate instead of the default release')
    options = parser.parse_args()
    if options.sdk is None:
        parser.error('Pass --sdk /path/to/vitasdk or set VITASDK')
    build(options.sdk, not options.debug)
