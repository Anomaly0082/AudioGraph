"""Assemble the locally built audio runtime WITH its matching source/notice bundle.

No downloads, global installation, or PATH FFmpeg lookup. Existing runtime is
renamed to a backup on --force. Source and license bytes remain unchanged.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import uuid
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def sha(path):
    with path.open('rb') as src:
        return hashlib.file_digest(src, 'sha256').hexdigest()


def regular(path):
    for part in (path, *path.parents):
        if part.is_symlink() or (hasattr(part, 'is_junction') and part.is_junction()):
            raise ValueError('Linked input is unsupported: ' + str(path))
    if not path.is_file():
        raise ValueError('Missing regular file: ' + str(path))


def run(executable, *args):
    result = subprocess.run([str(executable), *args], capture_output=True, stdin=subprocess.DEVNULL,
        creationflags=0x08000000 if os.name == 'nt' else 0, timeout=30)
    text = (result.stdout + result.stderr).decode('utf-8', 'replace')
    if result.returncode:
        raise RuntimeError(text[-4000:])
    return text


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')


def prepare(build, cache, msys, force):
    lock_path = ROOT / 'scripts/ffmpeg-audio.lock.json'
    lock = json.loads(lock_path.read_text(encoding='utf-8-sig'))
    sources = []
    for key in ('ffmpeg_source', 'lame_source'):
        item = lock[key]
        if Path(item['name']).name != item['name']:
            raise ValueError('Invalid source name')
        path = cache / item['name']; regular(path)
        if sha(path) != item['sha256']:
            raise ValueError('Source SHA-256 mismatch: ' + path.name)
        sources.append(path)
    runtime = ROOT / 'apps/desktop/src-tauri/resources/ffmpeg'
    if runtime.exists() and not force:
        raise FileExistsError('Runtime exists; --force preserves it as a backup before replacement')
    stage = build / ('prepared-' + uuid.uuid4().hex)
    (stage / 'bin').mkdir(parents=True)
    for path in (build / 'stage/bin').iterdir():
        regular(path)
        if path.suffix.lower() not in ('.exe', '.dll'):
            continue
        shutil.copyfile(path, stage / 'bin' / path.name)
    for name in ('ffmpeg.exe','ffprobe.exe','avcodec-62.dll','avformat-62.dll','avutil-60.dll','avfilter-11.dll','swresample-6.dll','libmp3lame-0.dll'):
        regular(stage / 'bin' / name)
    ffmpeg = stage / 'bin/ffmpeg.exe'
    configuration = run(ffmpeg, '-hide_banner', '-buildconf')
    license_report = run(ffmpeg, '-hide_banner', '-L')
    if any(flag in configuration for flag in ('--enable-gpl', '--enable-nonfree', '--enable-librav1e', '--enable-version3')):
        raise ValueError('Unexpected license/features in audio-only FFmpeg')
    for flag in ('--disable-network', '--disable-autodetect', '--disable-avdevice', '--disable-swscale', '--enable-shared', '--enable-libmp3lame'):
        if flag not in configuration:
            raise ValueError('Missing audio build option: ' + flag)
    if 'Lesser General Public License' not in license_report or 'version 2.1' not in license_report:
        raise ValueError('Unexpected FFmpeg license report')
    (stage / 'build-configuration.txt').write_text(configuration, encoding='utf-8')
    (stage / 'license-report.txt').write_text(license_report, encoding='utf-8')
    (stage / 'version.txt').write_text(run(ffmpeg, '-hide_banner', '-version'), encoding='utf-8')

    # Independent import walk: only packaged DLLs or known Windows system DLLs.
    system_dlls = {'kernel32.dll','msvcrt.dll','ucrtbase.dll','advapi32.dll','user32.dll','ws2_32.dll',
        'bcrypt.dll','secur32.dll','ole32.dll','shell32.dll','winmm.dll','ntdll.dll','psapi.dll','crypt32.dll','gdi32.dll'}
    binaries = list((stage / 'bin').iterdir())
    names = {path.name.lower() for path in binaries}
    imports = {}
    for path in binaries:
        pe = run(msys / 'mingw64/bin/objdump.exe', '-p', str(path))
        imported = sorted(set(re.findall(r'DLL Name:\s*(\S+)', pe)))
        if 'file format pei-x86-64' not in pe or not imported:
            raise ValueError('Not a verified Windows x64 PE import table: ' + path.name)
        for name in imported:
            lower = name.lower()
            if lower not in names and lower not in system_dlls and not lower.startswith(('api-ms-win-', 'ext-ms-win-')):
                raise ValueError('Unbundled non-system DLL: ' + name)
        imports[path.name] = imported
    write_json(stage / 'pe-imports.json', imports)

    for name in ('FFmpeg','LAME','GCC','MinGW','winpthreads'):
        (stage / 'licenses' / name).mkdir(parents=True)
    with zipfile.ZipFile(sources[0]) as archive:
        prefix = 'FFmpeg-' + lock['ffmpeg_source']['commit'] + '/'
        for name in ('COPYING.LGPLv2.1','COPYING.GPLv2','LICENSE.md','CREDITS'):
            (stage / 'licenses/FFmpeg' / name).write_bytes(archive.read(prefix + name))
    found = set()
    with tarfile.open(sources[1], 'r|xz') as archive:
        for member in archive:
            if member.isfile() and member.name.removeprefix('./') in ('COPYING','LICENSE'):
                name = member.name.removeprefix('./'); found.add(name)
                (stage / 'licenses/LAME' / name).write_bytes(archive.extractfile(member).read())
    if found != {'COPYING','LICENSE'}:
        raise ValueError('LAME license originals absent')
    license_root = msys / 'mingw64/share/licenses'
    for origin, target, selected in (
        ('gcc-libs','GCC',('COPYING.RUNTIME','COPYING3','COPYING.LIB','README')),
        ('crt','MinGW',('COPYING','COPYING.MinGW-w64-runtime.txt','COPYING.MinGW-w64.txt')),
        ('libwinpthread','winpthreads',('COPYING',))):
        for name in selected:
            path = license_root / origin / name; regular(path)
            shutil.copyfile(path, stage / 'licenses' / target / name)
    shutil.copyfile(ROOT / 'docs/ffmpeg-audio-NOTICE.txt', stage / 'NOTICE.txt')
    shutil.copyfile(lock_path, stage / 'upstream-lock.json')

    (stage / 'sources').mkdir()
    package = stage / 'sources/ffmpeg-audio-sources.zip'
    with zipfile.ZipFile(package, 'x', compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for path in sources:
            archive.write(path, 'sources/' + path.name, compress_type=zipfile.ZIP_STORED)
        for name in ('ffmpeg-audio.lock.json','build-ffmpeg-audio.ps1','build-ffmpeg-audio.sh','prepare-ffmpeg.ps1',
                     'prepare-ffmpeg-audio.py','ffmpeg-audio-distribution.ps1'):
            path = ROOT / 'scripts' / name; regular(path); archive.write(path, 'scripts/' + name)
        for path in sorted((ROOT / 'scripts').glob('ffmpeg-audio-*.sh')):
            regular(path); archive.write(path, 'scripts/' + path.name)
        for path in sorted((build / 'metadata').rglob('*')):
            if path.is_file():
                regular(path); archive.write(path, 'build-record/' + path.relative_to(build / 'metadata').as_posix())
        for path in sorted((stage / 'licenses').rglob('*')):
            if path.is_file(): archive.write(path, path.relative_to(stage).as_posix())
        for name in ('NOTICE.txt','build-configuration.txt','license-report.txt','version.txt','pe-imports.json'):
            archive.write(stage / name, name)
        archive.write(ROOT / 'docs/ffmpeg-audio-build.md', 'BUILD-INSTRUCTIONS.md')
    # Verify the source bytes stored in the companion ZIP, not just the input files.
    with zipfile.ZipFile(package) as archive:
        if archive.testzip() is not None:
            raise ValueError('Source package CRC failure')
        for path in sources:
            with archive.open('sources/' + path.name) as stream:
                if hashlib.file_digest(stream, 'sha256').hexdigest() != sha(path):
                    raise ValueError('Source bundle byte mismatch')
    files = [dict(name=p.name, bytes=p.stat().st_size, sha256=sha(p)) for p in sorted((stage / 'bin').iterdir())]
    distribution = [dict(path=p.relative_to(stage).as_posix(), bytes=p.stat().st_size, sha256=sha(p))
        for p in sorted(stage.rglob('*')) if p.is_file()]
    metadata = dict(schema=2, provider='AudioGraph audio-only build', variant=lock['profile'],
        ffmpeg_revision=lock['ffmpeg_source']['commit'], lame_revision=lock['lame_source']['svn_revision'],
        license='LGPL-2.1-or-later',
        redistribution_ready=True, dependency_sources_present=True, files=files, distribution_files=distribution,
        source_package='sources/ffmpeg-audio-sources.zip',
        redistribution_note='FFmpeg component materials prepared; preserve this complete folder when distributing. This is not a license audit of the entire AudioGraph application.',
        replacement_rights='The running application does not enforce hashes. Compatible FFmpeg/LAME libraries may be replaced and modified under their licenses.')
    write_json(stage / 'bundle-provenance.json', metadata)
    runtime.parent.mkdir(parents=True, exist_ok=True)
    backup = runtime.with_name('ffmpeg.previous-' + uuid.uuid4().hex)
    existed = runtime.exists()
    if existed:
        if runtime.is_symlink() or runtime.is_junction(): raise ValueError('Linked runtime cannot be replaced')
        runtime.rename(backup)
    try:
        stage.rename(runtime)
    except OSError:
        if existed: backup.rename(runtime)
        raise
    print(json.dumps(dict(runtime=str(runtime), backup=str(backup) if existed else None,
        runtime_bin_bytes=sum(x['bytes'] for x in files), source_package_bytes=(runtime / metadata['source_package']).stat().st_size), indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build-directory', type=Path, default=ROOT / 'build/ffmpeg-audio')
    parser.add_argument('--cache', type=Path, default=ROOT / 'build/ffmpeg-cache')
    parser.add_argument('--msys-root', type=Path, required=True)
    parser.add_argument('--force', action='store_true')
    args = parser.parse_args()
    prepare(args.build_directory.resolve(), args.cache.resolve(), args.msys_root.resolve(), args.force)
