#!/usr/bin/env python3
"""Regression checks against the owner's local, verified APK; game bytes stay local."""
import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

from inspect_apk import RUNTIME_ASSETS, decode_swf, swf_tags
from prepare_runtime import (GAME_SETTINGS_PATCH, abc_bytes, abc_tags, encode_tag,
                             extract_runtime_assets, patch_game_settings, prepare, runtime_support)

ROOT = Path(__file__).resolve().parents[1]


class SwfTagsTests(unittest.TestCase):
    def test_truncated_headers_payload_and_missing_end_are_rejected(self):
        for data in (b'\x00', b'\xff\x03', encode_tag(82, b'abc')[:-1],
                     encode_tag(82, b'abc')):
            with self.subTest(data=data), self.assertRaises(ValueError):
                list(swf_tags(data, 0))

    def test_end_payload_and_trailing_bytes_are_rejected(self):
        for data in (encode_tag(0, b'x'), encode_tag(0, b'') + b'x'):
            with self.subTest(data=data), self.assertRaises(ValueError):
                list(swf_tags(data, 0))

    def test_valid_short_and_long_tags(self):
        body = struct.pack('<H', (1 << 6) | 1) + b'x' + encode_tag(0, b'')
        self.assertEqual(list(swf_tags(body, 0)), [(1, b'x'), (0, b'')])


class RuntimeResourcesTests(unittest.TestCase):
    def test_all_original_locales_are_extracted_and_hash_checked(self):
        import zipfile
        inventory = json.loads((ROOT / 'tools/inputs.lock.json').read_text())
        with tempfile.TemporaryDirectory(dir=ROOT / 'build') as directory:
            directory = Path(directory)
            paths = extract_runtime_assets(APK, inventory, directory)
            self.assertEqual({path.name for path in paths}, set(RUNTIME_ASSETS))
            with zipfile.ZipFile(APK) as apk:
                for path in paths:
                    self.assertEqual(path.read_bytes(), apk.read('assets/' + path.name))
            inventory['inputs'][next(i for i, entry in enumerate(inventory['inputs'])
                                     if entry['path'] == 'assets/text_en.xml')]['sha256'] = '0' * 64
            with self.assertRaisesRegex(ValueError, 'differs from inventory'):
                extract_runtime_assets(APK, inventory, directory / 'rejected')
            self.assertFalse((directory / 'rejected').exists())

    def test_asset_symlink_is_rejected_without_changing_the_target(self):
        inventory = json.loads((ROOT / 'tools/inputs.lock.json').read_text())
        with tempfile.TemporaryDirectory(dir=ROOT / 'build') as directory:
            directory = Path(directory)
            target = directory / 'preserve.txt'
            target.write_text('existing data')
            (directory / 'text_ru.xml').symlink_to(target)
            with self.assertRaisesRegex(ValueError, 'symlink'):
                extract_runtime_assets(APK, inventory, directory)
            self.assertEqual(target.read_text(), 'existing data')
            self.assertFalse((directory / 'text_en.xml').exists())


class RuntimePreparationTests(unittest.TestCase):
    def test_failed_air_compilation_does_not_reuse_old_bytecode(self):
        previous, _ = runtime_support()
        artifact = ROOT / 'build/runtime/lonewolf-platform.abc'
        with self.assertRaisesRegex(ValueError, 'compilation failed'):
            runtime_support(ROOT / 'tools/testdata/InvalidPermissionEvent.as')
        self.assertEqual(artifact.read_bytes(), previous)

    def test_outputs_cannot_overwrite_input_or_lock_through_symlinks(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'build') as directory:
            directory = Path(directory)
            for name, protected in [('linked.swf', APK),
                                    ('linked.json', ROOT / 'tools/inputs.lock.json')]:
                link = directory / name
                link.symlink_to(protected)
                with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'overwrite'):
                    prepare(APK, ROOT / 'tools/inputs.lock.json', directory / 'linked.swf')
                link.unlink()

    def test_real_avm2_bootstrap_and_failure_with_logging_disabled(self):
        if PROBE is None:
            self.skipTest('Pass --probe to exercise the built AVM2 runtime')
        import zipfile
        with tempfile.TemporaryDirectory(dir=ROOT / 'build') as directory:
            directory = Path(directory)
            prepared = directory / 'prepared.swf'
            prepare(APK, ROOT / 'tools/inputs.lock.json', prepared)
            original = directory / 'original.swf'
            with zipfile.ZipFile(APK) as apk:
                original.write_bytes(apk.read('assets/Lonewolf.swf'))
            environment = dict(os.environ, RUST_LOG='off')
            good = subprocess.run([str(PROBE), str(prepared)], capture_output=True,
                                  text=True, timeout=45, env=environment)
            self.assertEqual(good.returncode, 0, good.stderr)
            self.assertIn('name: the_game,', good.stdout)
            self.assertIn('name: introClip,', good.stdout)
            body, offset = decode_swf(prepared.read_bytes())
            tags = list(swf_tags(body, offset))
            insertion = next(i for i, (tag, _) in enumerate(tags) if tag == 82)
            tags.insert(insertion, (82, struct.pack('<I', 1) + b'invalid\0\0'))
            malformed_body = body[:offset] + b''.join(encode_tag(tag, data) for tag, data in tags)
            malformed = directory / 'malformed.swf'
            malformed.write_bytes(b'FWS' + bytes([43]) +
                                  struct.pack('<I', len(malformed_body) + 8) + malformed_body)
            damaged = subprocess.run([str(PROBE), str(malformed)], capture_output=True,
                                     text=True, timeout=45, env=environment)
            self.assertNotEqual(damaged.returncode, 0, damaged.stdout)
            self.assertIn('runtime errors', damaged.stderr)
            bad = subprocess.run([str(PROBE), str(original)], capture_output=True,
                                 text=True, timeout=45, env=environment)
            self.assertNotEqual(bad.returncode, 0)
            self.assertIn('Bootstrap incomplete: 1 runtime errors', bad.stderr)
            self.assertNotIn('name: the_game,', bad.stdout)

    def test_verified_bundle_changes_only_settings_initializer_and_added_runtime(self):
        import zipfile
        with tempfile.TemporaryDirectory(dir=ROOT / 'build') as directory:
            output = Path(directory) / 'runtime.swf'
            report = prepare(APK, ROOT / 'tools/inputs.lock.json', output)
            first_result = output.read_bytes()
            prepare(APK, ROOT / 'tools/inputs.lock.json', output)
            self.assertEqual(first_result, output.read_bytes())
            with zipfile.ZipFile(APK) as apk:
                original = apk.read('assets/Lonewolf.swf')
            old_body, old_offset = decode_swf(original)
            new_body, new_offset = decode_swf(first_result)
            old_tags = list(swf_tags(old_body, old_offset))
            new_tags = list(swf_tags(new_body, new_offset))
            insertion = next(i for i, (tag, _) in enumerate(old_tags) if tag == 82)
            self.assertEqual(old_body[:old_offset], new_body[:new_offset])
            retained = new_tags[:insertion] + new_tags[insertion + 89:]
            self.assertEqual(retained[:insertion], old_tags[:insertion])
            self.assertEqual(retained[insertion + 1:], old_tags[insertion + 1:])
            self.assertEqual(retained[insertion], (82, patch_game_settings(old_tags[insertion][1])))
            old = abc_bytes(old_tags[insertion][1])
            new = abc_bytes(retained[insertion][1])
            self.assertEqual(new[:464493], old[:464493])
            self.assertEqual(new[464493:464495], bytes.fromhex('f533'))
            self.assertEqual(new[464495:471129], old[464495:471129])
            self.assertEqual(new[471129:471139], bytes.fromhex('d026618927d027618c27'))
            self.assertEqual(new[471139:], old[471129:])
            self.assertEqual(report['game_settings_patch'], GAME_SETTINGS_PATCH)
            with self.assertRaisesRegex(ValueError, 'Unexpected game bytecode'):
                patch_game_settings(retained[insertion][1])
            self.assertEqual(report['extension_abc_count'], 88)
            self.assertEqual(report['runtime_abc_count'], 1)
            self.assertEqual(len(report['excluded_adapters']), 5)

    def test_different_input_lock_is_rejected_before_output(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'build') as directory:
            directory = Path(directory)
            inventory = json.loads((ROOT / 'tools/inputs.lock.json').read_text())
            inventory['apk']['sha256'] = '0' * 64
            lock = directory / 'wrong.json'
            lock.write_text(json.dumps(inventory))
            output = directory / 'must-not-exist.swf'
            with self.assertRaisesRegex(ValueError, 'recorded inputs'):
                prepare(APK, lock, output)
            self.assertFalse(output.exists())


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('apk', type=Path)
    parser.add_argument('--probe', type=Path, help='Built host binary for actual AVM2 execution')
    args = parser.parse_args()
    APK, PROBE = args.apk.resolve(), args.probe.resolve() if args.probe else None
    (ROOT / 'build').mkdir(exist_ok=True)
    unittest.main(argv=[sys.argv[0]])
