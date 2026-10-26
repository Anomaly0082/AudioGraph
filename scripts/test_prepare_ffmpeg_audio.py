"""Synthetic packaging tests; no FFmpeg execution, downloads, or real runtime writes."""
import contextlib
import importlib.util
import io
import json
import sys
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location('prepare_audio', Path(__file__).with_name('prepare-ffmpeg-audio.py'))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PrepareTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.build = self.root / 'build'
        self.cache = self.root / 'cache'
        self.msys = self.root / 'msys'
        self.runtime = self.root / 'apps/desktop/src-tauri/resources/ffmpeg'
        for path in (self.build / 'stage/bin', self.cache, self.root / 'scripts', self.root / 'docs'):
            path.mkdir(parents=True)
        self.sources = {}
        ffmpeg = self.cache / 'ffmpeg.zip'
        with zipfile.ZipFile(ffmpeg, 'w') as archive:
            for name in ('COPYING.LGPLv2.1', 'COPYING.GPLv2', 'LICENSE.md', 'CREDITS'):
                archive.writestr('FFmpeg-fixture/' + name, ('original ' + name).encode())
        lame = self.cache / 'lame.tar.xz'
        with tarfile.open(lame, 'w:xz') as archive:
            for name in ('COPYING', 'LICENSE'):
                content = ('original lame ' + name).encode()
                member = tarfile.TarInfo(name)
                member.size = len(content)
                archive.addfile(member, io.BytesIO(content))
        for key, path in (('ffmpeg_source', ffmpeg), ('lame_source', lame)):
            self.sources[key] = {'name': path.name, 'sha256': MODULE.sha(path)}
        self.sources['ffmpeg_source']['commit'] = 'fixture'
        self.sources['lame_source']['svn_revision'] = 'fixture'
        self.lock = dict(self.sources, profile='fixture')
        (self.root / 'scripts/ffmpeg-audio.lock.json').write_text(json.dumps(self.lock), encoding='utf-8')
        for name in ('build-ffmpeg-audio.ps1', 'build-ffmpeg-audio.sh', 'prepare-ffmpeg.ps1', 'prepare-ffmpeg-audio.py', 'ffmpeg-audio-distribution.ps1'):
            (self.root / 'scripts' / name).write_text('fixture recipe', encoding='utf-8')
        for name in ('ffmpeg-audio-NOTICE.txt', 'ffmpeg-audio-build.md'):
            (self.root / 'docs' / name).write_text('fixture notice', encoding='utf-8')
        for origin, names in (
            ('gcc-libs', ('COPYING.RUNTIME', 'COPYING3', 'COPYING.LIB', 'README')),
            ('crt', ('COPYING', 'COPYING.MinGW-w64-runtime.txt', 'COPYING.MinGW-w64.txt')),
            ('libwinpthread', ('COPYING',))):
            directory = self.msys / 'mingw64/share/licenses' / origin
            directory.mkdir(parents=True)
            for name in names:
                (directory / name).write_bytes(b'original toolchain license')
        for name in ('ffmpeg.exe', 'ffprobe.exe', 'avcodec-62.dll', 'avformat-62.dll', 'avutil-60.dll', 'avfilter-11.dll', 'swresample-6.dll', 'libmp3lame-0.dll'):
            (self.build / 'stage/bin' / name).write_bytes(b'fixture PE')
        self.imports = 'file format pei-x86-64\nDLL Name: KERNEL32.dll\n'

    def fake_run(self, executable, *args):
        if args == ('-hide_banner', '-buildconf'):
            return '--disable-network --disable-autodetect --disable-avdevice --disable-swscale --enable-shared --enable-libmp3lame'
        if args == ('-hide_banner', '-L'):
            return 'GNU Lesser General Public License version 2.1'
        if args[0] == '-p':
            return self.imports
        return 'fixture version'

    def prepare(self, force=False):
        with patch.object(MODULE, 'ROOT', self.root), patch.object(MODULE, 'run', self.fake_run), contextlib.redirect_stdout(io.StringIO()):
            MODULE.prepare(self.build, self.cache, self.msys, force)

    def old_runtime(self):
        self.runtime.mkdir(parents=True)
        (self.runtime / 'user-replacement.dll').write_bytes(b'user replacement')

    def test_existing_runtime_requires_force_and_remains_unchanged(self):
        self.old_runtime()
        with self.assertRaises(FileExistsError):
            self.prepare()
        self.assertEqual((self.runtime / 'user-replacement.dll').read_bytes(), b'user replacement')
        self.assertEqual(list(self.runtime.parent.glob('ffmpeg.previous-*')), [])

    def test_force_preserves_previous_runtime_and_embeds_exact_sources(self):
        self.old_runtime()
        self.prepare(True)
        backups = list(self.runtime.parent.glob('ffmpeg.previous-*'))
        self.assertEqual(len(backups), 1)
        self.assertEqual((backups[0] / 'user-replacement.dll').read_bytes(), b'user replacement')
        with zipfile.ZipFile(self.runtime / 'sources/ffmpeg-audio-sources.zip') as archive:
            for item in self.sources.values():
                self.assertEqual(archive.read('sources/' + item['name']), (self.cache / item['name']).read_bytes())
            for name in ('build-ffmpeg-audio.sh', 'build-ffmpeg-audio.ps1', 'ffmpeg-audio.lock.json'):
                self.assertEqual(archive.read('scripts/' + name), (self.root / 'scripts' / name).read_bytes())

    def test_bad_source_hash_fails_before_replacing_runtime(self):
        self.old_runtime()
        (self.cache / self.sources['ffmpeg_source']['name']).write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'SHA-256 mismatch'):
            self.prepare(True)
        self.assertEqual((self.runtime / 'user-replacement.dll').read_bytes(), b'user replacement')

    def test_unknown_import_fails_before_replacing_runtime(self):
        self.old_runtime()
        self.imports += 'DLL Name: unknown-third-party.dll\n'
        with self.assertRaisesRegex(ValueError, 'Unbundled non-system DLL'):
            self.prepare(True)
        self.assertEqual((self.runtime / 'user-replacement.dll').read_bytes(), b'user replacement')

    def test_invalid_pe_or_empty_imports_fail_before_replacing_runtime(self):
        self.old_runtime()
        for output in ('', 'file format pei-x86-64\n', 'file format pei-i386\nDLL Name: KERNEL32.dll\n'):
            with self.subTest(output=output):
                self.imports = output
                with self.assertRaisesRegex(ValueError, 'Not a verified Windows x64 PE import table'):
                    self.prepare(True)
                self.assertEqual((self.runtime / 'user-replacement.dll').read_bytes(), b'user replacement')
                self.assertEqual(list(self.runtime.parent.glob('ffmpeg.previous-*')), [])


if __name__ == '__main__':
    unittest.main()
