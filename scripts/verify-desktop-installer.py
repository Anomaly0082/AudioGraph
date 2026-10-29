"""Inspect and extract an NSIS installer as data; NEVER execute install/uninstall.

Checks the exact compiled script, payload hashes and a deletion-list model.
Actual Windows installer lifecycle/WebView2 installation still needs an isolated VM.
"""
import argparse
import hashlib
import json
from pathlib import Path, PureWindowsPath
import re
import subprocess
import zipfile


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream,'sha256').hexdigest()


def relative(name):
    p=PureWindowsPath(name)
    if p.is_absolute() or p.drive or '..' in p.parts or ':' in name or '*' in name or '?' in name:
        raise ValueError('Unsafe archive/deletion path: '+name)
    return p.as_posix()


def script_policy(script, expected):
    assert re.search(r'!define BUNDLEID "com\.audioprocess\.prototype"',script)
    assert '!define INSTALLMODE "currentUser"' in script
    assert '!define INSTALLWEBVIEW2MODE "embedBootstrapper"' in script
    assert '!define MUI_FINISHPAGE_RUN_NOTCHECKED' in script
    for forbidden in (r'DeleteAppData',r'(?im)^\s*RmDir\s+/r\b',r'(?im)^\s*(?:Delete|RmDir).*\$(?:APPDATA|LOCALAPPDATA)',
                      r'!insertmacro CheckIfAppIsRunning',r'nsis_tauri_utils::KillProcess',r'wix_loop:',r'Delete "\$INSTDIR\\\$OldMainBinaryName"'):
        assert not re.search(forbidden,script), forbidden
    assert script.count('!insertmacro AudioGraphRequireClosed')==3
    webview=script.split('Section WebView2',1)[1].split('SectionEnd',1)[0]
    assert '$UpdateMode' not in webview
    assert 'InitPluginsDir' in webview and '$PLUGINSDIR\\MicrosoftEdgeWebview2Setup.exe' in webview
    assert "ExecWait '\"$6\" ${WEBVIEW2INSTALLERARGS} /install' $1" in webview
    assert 'ReadRegStr $4 HKCU' in webview and 'ReadRegStr $4 HKLM' in webview
    assert '$4 == "0.0.0.0"' in webview
    main=re.search(r'!define MAINBINARYNAME "([^"]+)"',script).group(1)
    uninstall=script.split('Section Uninstall',1)[1].split('SectionEnd',1)[0]
    delete_paths=[]
    for path in re.findall(r'(?m)^\s*Delete "\$INSTDIR\\([^"]+)"',uninstall):
        path=path.replace('${MAINBINARYNAME}',main)
        assert '$' not in path
        delete_paths.append(relative(path))
    assert set(delete_paths)==set(expected)|{'uninstall.exe'}, 'Uninstall ownership differs from installed files'
    # Model the literal Delete list. No simulated or real user files are deleted.
    sentinels={'workspace/input.wav','workspace/graph.json','notes.txt','ffmpeg/user-notes.txt'}
    for _scenario in ('uninstall','upgrade-uninstall-then-install','repair-overwrite'):
        remaining=(set(expected)|sentinels)-set(delete_paths)
        assert sentinels<=remaining
    return delete_paths


def verify(installer, output, sevenzip):
    if output.exists(): raise FileExistsError('Choose a new inspection directory')
    record=json.loads(Path(str(installer)+'.build.json').read_text(encoding='utf-8-sig'))
    assert sha(installer)==record['sha256']
    stage=Path(record['stage'])
    source=stage/'installer.nsi'
    assert sha(source)==record['generated_script_sha256'].lower()
    rows=json.loads((stage/'expected-payload.json').read_text(encoding='utf-8-sig'))
    expected={relative(row['path']):row for row in rows}
    assert len(expected)==len(rows)
    script=source.read_text(encoding='utf-8-sig')
    removed=script_policy(script,expected)
    def archive_command(*args):
        result=subprocess.run([str(sevenzip),*args],capture_output=True,text=True,encoding='utf-8',errors='replace',timeout=90,
                              creationflags=0x08000000)
        if result.returncode: raise RuntimeError(result.stdout+result.stderr)
        return result.stdout
    listing=archive_command('l','-slt','-sccUTF-8',str(installer))
    entries=listing.split('----------',1)[1]
    for name in re.findall(r'(?m)^Path = (.+)$',entries): relative(name.strip())
    output.mkdir(parents=True)
    extract=output/'payload'
    archive_command('x','-y','-sccUTF-8','-o'+str(extract),str(installer))
    for path,row in expected.items():
        file=extract/path
        assert file.is_file() and file.stat().st_size==row['bytes'],path
        assert sha(file)==row['sha256'],path
    bootstrap=extract/'$PLUGINSDIR/MicrosoftEdgeWebview2Setup.exe'
    assert sha(bootstrap)==record['bootstrapper']['sha256'].lower()
    with zipfile.ZipFile(extract/'ffmpeg/sources/ffmpeg-audio-sources.zip') as archive:
        assert archive.testzip() is None
    report=dict(passed=True,installer=str(installer),sha256=sha(installer),bytes=installer.stat().st_size,
        installed_payload_files=len(expected),uninstall_owned_files=len(removed),bootstrap_matches_verified_microsoft_file=True,
        installer_executed=False,uninstaller_executed=False,desktop_or_registry_changed=False,
        validation='Compiled-script policy + extracted payload hashes + in-memory deletion model',
        not_verified=['Real installation/upgrade/uninstall','Missing-WebView2 download/install in a clean VM','Native installer page appearance'])
    (output/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps(report,ensure_ascii=False,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for flag in ('installer','output','sevenzip'): parser.add_argument('--'+flag,type=Path,required=True)
    args=parser.parse_args()
    verify(args.installer.resolve(),args.output.resolve(),args.sevenzip.resolve())
