"""Small synthetic offline regression tests; no real source-cache scan."""
import contextlib
import hashlib
import importlib.util
import io
import json
import sys
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('collector', Path(__file__).with_name('collect-ffmpeg-notices.py'))
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)


def tar_bytes(entries, mode):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode=mode) as archive:
        for name, data, target in entries:
            item = tarfile.TarInfo(name)
            if target is not None:
                item.type = tarfile.SYMTYPE
                item.linkname = target
                archive.addfile(item)
            else:
                item.size = len(data)
                archive.addfile(item, io.BytesIO(data))
    return stream.getvalue()


class CollectionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.cache = self.root / 'cache'
        self.cache.mkdir()
        self.runtime = self.root / 'runtime'
        self.runtime.mkdir()
        for name in ('build-configuration.txt', 'license-report.txt', 'bundle-provenance.json'):
            (self.runtime / name).write_text('--enable-demo', encoding='utf-8')
        self.output = self.root / 'output'
        self.lock = self.root / 'lock.json'
        self.notice = b'Copyright example\r\nLicense text\xff\x00\n'
        self.archive_name = '20-demo_' + 'a' * 64 + '.tar.xz'

    def fixture(self, outer=None, ffmpeg=None, inner=None, builder=None):
        inner = inner if inner is not None else [('project/LICENSE', self.notice, None), ('project/NOTICE', self.notice, None)]
        source = tar_bytes(inner, 'w:xz')
        if outer is None:
            outer = [('.cache/downloads/' + self.archive_name, source, None), ('.cache/downloads/20-demo.tar.xz', b'', self.archive_name)]
        archives = {
            'ffmpeg_source': ('ffmpeg.zip', ffmpeg or [('ffmpeg/COPYING', self.notice)]),
            'builder_source': ('builder.zip', builder if builder is not None else [('builder/scripts.d/20-demo.sh', b'SCRIPT_COMMIT="' + b'b' * 40 + b'"\n# --enable-demo\n')]),
            'dependency_sources': ('dependencies.zip', [('cache.tar.gz', tar_bytes(outer, 'w:gz'))]),
        }
        pin = {}
        for component, (name, entries) in archives.items():
            path = self.cache / name
            with zipfile.ZipFile(path, 'w') as archive:
                for filename, data in entries:
                    archive.writestr(filename, data)
            pin[component] = {'name': name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        self.lock.write_text(json.dumps(pin), encoding='utf-8')
        return pin, source

    def collect(self):
        with contextlib.redirect_stdout(io.StringIO()):
            collector.collect(self.cache, self.output, self.lock, self.runtime)

    def test_hashes_dedup_original_bytes_and_no_release_approval(self):
        pin, source = self.fixture()
        self.collect()
        manifest = json.loads((self.output / 'collection.json').read_text())
        self.assertIs(manifest['redistribution_ready'], False)
        self.assertEqual(manifest['dependency_archives'], 1)
        self.assertEqual(manifest['notice_files'], 3)
        self.assertEqual(manifest['unique_notice_contents'], 1)
        sha = hashlib.sha256(self.notice).hexdigest()
        self.assertEqual((self.output / 'licenses' / (sha + '.txt')).read_bytes(), self.notice)
        notices = json.loads((self.output / 'inventory/notices.json').read_text())
        self.assertEqual({n['original_path'] for n in notices}, {'ffmpeg/COPYING', 'project/LICENSE', 'project/NOTICE'})
        self.assertEqual({n['sha256'] for n in notices}, {sha})
        components = json.loads((self.output / 'inventory/components.json').read_text())
        self.assertEqual(components[0]['source_sha256'], hashlib.sha256(source).hexdigest())
        self.assertNotEqual(components[0]['source_sha256'], 'a' * 64)
        for asset in pin.values():
            self.assertEqual((self.output / 'sources' / asset['name']).read_bytes(), (self.cache / asset['name']).read_bytes())
        self.assertFalse((self.output / 'source-scan.tmp.tar.xz').exists())

    def test_rejects_corrupted_pin_before_output_creation(self):
        self.fixture()
        with (self.cache / 'ffmpeg.zip').open('ab') as stream:
            stream.write(b'changed')
        with self.assertRaisesRegex(ValueError, 'SHA-256 mismatch'):
            self.collect()
        self.assertFalse(self.output.exists())

    def test_existing_output_is_never_overwritten(self):
        self.fixture()
        self.output.mkdir()
        marker = self.output / 'marker'
        marker.write_bytes(b'keep')
        with self.assertRaises(FileExistsError):
            self.collect()
        self.assertEqual(list(self.output.iterdir()), [marker])
        self.assertEqual(marker.read_bytes(), b'keep')

    def test_rejects_unsafe_paths(self):
        for name in ('../LICENSE', '/LICENSE', 'C:/LICENSE', 'root\\LICENSE', 'a/../../LICENSE', 'a\x00b'):
            with self.subTest(name=name), self.assertRaises(ValueError):
                collector.safe_parts(name)

    def test_zip_traversal_rejected_without_extracting(self):
        self.fixture(ffmpeg=[('../outside-LICENSE', self.notice)])
        with self.assertRaises(ValueError):
            self.collect()
        self.assertFalse((self.root / 'outside-LICENSE').exists())

    def test_inner_tar_traversal_rejected_and_scratch_removed(self):
        self.fixture(inner=[('../LICENSE', self.notice, None)])
        with self.assertRaises(ValueError):
            self.collect()
        self.assertFalse((self.output / 'source-scan.tmp.tar.xz').exists())

    def test_alias_target_must_be_flat_and_existing(self):
        for target in ('../outside', 'sub/file', 'missing.tar.xz'):
            with self.subTest(target=target):
                self.fixture(outer=[('.cache/downloads/demo.tar.xz', b'', target)])
                # Each failing run leaves partial evidence, never a completed manifest.
                self.output = self.root / ('output-' + hashlib.sha256(target.encode()).hexdigest())
                with self.assertRaises(ValueError):
                    self.collect()
                self.assertFalse((self.output / 'collection.json').exists())

    def test_source_symlinks_are_not_followed(self):
        self.fixture(inner=[('project/LICENSE', b'', '../../outside'), ('project/COPYING', self.notice, None)])
        self.collect()
        rows = json.loads((self.output / 'inventory/notices.json').read_text())
        self.assertNotIn('project/LICENSE', [row['original_path'] for row in rows])

    def test_linux_colon_names_are_metadata_only(self):
        name = 'project/0x46d:sample/licenses/MIT.txt'
        self.fixture(inner=[(name, self.notice, None), ('project/0x46d:sample/input.raw', b'raw', None)])
        self.collect()
        rows = json.loads((self.output / 'inventory/notices.json').read_text())
        row = next(row for row in rows if row['original_path'] == name)
        self.assertEqual((self.output / row['file']).read_bytes(), self.notice)
        self.assertEqual(row['file'], 'licenses/' + hashlib.sha256(self.notice).hexdigest() + '.txt')
        self.assertFalse((self.output / 'project').exists())

    def test_oversized_zip_notice_is_recorded(self):
        self.fixture(ffmpeg=[('ffmpeg/LICENSE.large', b'x' * 65)])
        with mock.patch.object(collector, 'MAX_NOTICE', 64):
            self.collect()
        skipped = json.loads((self.output / 'inventory/oversized-zip-notices.json').read_text())
        self.assertEqual(skipped, [{'archive': 'ffmpeg.zip', 'path': 'ffmpeg/LICENSE.large', 'bytes': 65}])
        rows = json.loads((self.output / 'inventory/notices.json').read_text())
        self.assertNotIn('ffmpeg/LICENSE.large', [row['original_path'] for row in rows])

    def test_recipe_flatten_collision_rejected_without_overwrite(self):
        self.fixture(builder=[('builder/scripts.d/a__b.sh', b'first'), ('builder/scripts.d/a/b.sh', b'second')])
        with self.assertRaises(FileExistsError):
            self.collect()
        self.assertEqual((self.output / 'recipes/scripts.d__a__b.sh').read_bytes(), b'first')
        self.assertFalse((self.output / 'collection.json').exists())

    def test_enable_flags_only_come_from_ffbuild_configure_body(self):
        script = b'''# --enable-outside-comment
ffbuild_build() {
    echo --enable-build-only
}
ffbuild_configure() {
    echo --enable-demo --enable-demo --enable-second
}
ffbuild_unconfigure() {
    echo --enable-unconfigure-only
}
'''
        self.fixture(builder=[('builder/scripts.d/20-demo.sh', script)])
        (self.runtime / 'build-configuration.txt').write_text('--enable-demo --enable-second --enable-build-only --enable-outside-comment --enable-unconfigure-only', encoding='utf-8')
        self.collect()
        components = json.loads((self.output / 'inventory/components.json').read_text())
        self.assertEqual(components[0]['recipes'][0]['enable_flags'], ['--enable-demo', '--enable-second'])
        self.assertEqual(components[0]['observed_enable_flags'], ['--enable-demo', '--enable-second'])

    def test_script_without_configure_has_no_enable_flag_mapping(self):
        self.fixture(builder=[('builder/scripts.d/20-demo.sh', b'echo --enable-demo\n')])
        self.collect()
        components = json.loads((self.output / 'inventory/components.json').read_text())
        self.assertEqual(components[0]['observed_enable_flags'], [])

    def scan_fixture(self, entries, stage):
        path = self.root / 'tiny.tar.xz'
        path.write_bytes(tar_bytes(entries, 'w:xz'))
        if not self.output.exists():
            self.output.mkdir()
            (self.output / 'licenses').mkdir()
        notices = collector.Notices(self.output)
        details = collector.scan_tar(path, stage, notices)
        return notices, details

    def test_ffnvcodec_header_terms_preserved_whole_for_fixed_stage(self):
        name = './include/ffnvcodec/nvEncodeAPI.h'
        content = b'/* Copyright NVIDIA; permission terms */\r\n#define API 1\n'
        entries = [(name, content, None), ('./include/other/skip.h', b'other', None), ('./include/ffnvcodec/skip.c', b'code', None)]
        notices, _ = self.scan_fixture(entries, '50-ffnvcodec')
        self.assertEqual(len(notices.rows), 1)
        self.assertEqual(notices.rows[0]['original_path'], name)
        self.assertEqual((self.output / notices.rows[0]['file']).read_bytes(), content)
        notices, _ = self.scan_fixture(entries, 'other-stage')
        self.assertEqual(notices.rows, [])

    def test_ffnvcodec_header_size_limit_is_recorded(self):
        name = './include/ffnvcodec/nvEncodeAPI.h'
        with mock.patch.object(collector, 'MAX_NOTICE', 8):
            notices, details = self.scan_fixture([(name, b'x' * 9, None)], '50-ffnvcodec')
        self.assertEqual(notices.rows, [])
        self.assertEqual(details['oversized_notices'], [name])

    def test_glib_proxy_only_collects_regular_notices_verbatim(self):
        nested_name = './subprojects/packagecache/proxy-libintl-0.5.tar.gz'
        inner = tar_bytes([('proxy/COPYING', self.notice, None), ('proxy/src/main.c', b'code', None), ('proxy/LICENSE.link', b'', '../../outside')], 'w:gz')
        notices, details = self.scan_fixture([(nested_name, inner, None)], '10-glib')
        self.assertEqual(details['nested_archives'], [nested_name])
        self.assertEqual(len(notices.rows), 1)
        self.assertEqual(notices.rows[0]['original_path'], nested_name + '!proxy/COPYING')
        self.assertEqual((self.output / notices.rows[0]['file']).read_bytes(), self.notice)
        notices, _ = self.scan_fixture([(nested_name, inner, None)], 'other-stage')
        self.assertEqual(notices.rows, [])

    def test_glib_proxy_child_notice_limit_recorded(self):
        nested_name = './subprojects/packagecache/proxy-libintl-0.5.tar.gz'
        inner = tar_bytes([('proxy/LICENSE', b'x' * 9, None)], 'w:gz')
        with mock.patch.object(collector, 'MAX_NOTICE', 8):
            notices, details = self.scan_fixture([(nested_name, inner, None)], '10-glib')
        self.assertEqual(notices.rows, [])
        self.assertEqual(details['oversized_notices'], [nested_name + '!proxy/LICENSE'])

    def test_glib_proxy_expansion_limit_rejects_before_reading_child(self):
        nested_name = './subprojects/packagecache/proxy-libintl-0.5.tar.gz'
        inner = tar_bytes([('proxy/LICENSE', b'small', None)], 'w:gz')
        path = self.root / 'tiny.tar.xz'
        path.write_bytes(tar_bytes([(nested_name, inner, None)], 'w:xz'))
        self.output.mkdir()
        (self.output / 'licenses').mkdir()
        notices = collector.Notices(self.output)
        child = tarfile.TarInfo('proxy/LICENSE')
        child.size = 64 * 1024 * 1024 + 1
        real_open = tarfile.open
        nested = mock.MagicMock()
        nested.__enter__.return_value = [child]
        def open_archive(*args, **kwargs):
            return nested if 'fileobj' in kwargs else real_open(*args, **kwargs)
        with mock.patch.object(collector.tarfile, 'open', side_effect=open_archive):
            with self.assertRaisesRegex(ValueError, 'Nested source expansion too large'):
                collector.scan_tar(path, '10-glib', notices)
        self.assertEqual(notices.rows, [])

    def test_notice_selection(self):
        for name in ('COPYING.LGPL', 'LICENSE-MIT', 'Copyright', 'src/licenses/MIT.txt'):
            self.assertTrue(collector.is_notice(name), name)
        for name in ('src/main.c', '.git/LICENSE', 'licensee.c'):
            self.assertFalse(collector.is_notice(name), name)


if __name__ == '__main__':
    unittest.main()
