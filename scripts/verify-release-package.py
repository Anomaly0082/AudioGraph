"""Relocated Release package checks. Never launch the Tauri/WebView window.

Uses only synthetic audio and a supplied trusted SDK example plugin. Native
children run hidden with a system-only PATH and private working/data folders.
This is NOT a clean-machine/VM test or a WebView rendering test.
"""
import argparse
import ctypes
import hashlib
import json
import math
import os
from pathlib import Path
import queue
import re
import shutil
import struct
import subprocess
import threading
import time
import wave
import zipfile

SYSTEM_DLLS = set('kernel32 ntdll msvcrt ucrtbase advapi32 user32 gdi32 ole32 oleaut32 shell32 shlwapi comctl32 dwmapi ws2_32 bcrypt bcryptprimitives crypt32 secur32 userenv winmm version propsys uxtheme setupapi cfgmgr32 rpcrt4 wintrust imm32 powrprof iphlpapi shcore wtsapi32'.split())
HIDDEN = 0x08000000 if os.name == 'nt' else 0


def sha(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def run(args, *, cwd=None, env=None):
    result = subprocess.run([str(x) for x in args], cwd=cwd, env=env, stdin=subprocess.DEVNULL,
        capture_output=True, timeout=40, creationflags=HIDDEN)
    if result.returncode:
        raise RuntimeError(f'{args[0]} exited {result.returncode}: {result.stderr.decode("utf-8", "replace")[-2000:]}')
    return result.stdout.decode('utf-8', 'replace')


def audit_pe(path, root, dumpbin):
    headers = run([dumpbin, '/HEADERS', path])
    if not re.search(r'8664\s+machine', headers, re.I):
        raise ValueError('Not x64 PE: ' + str(path))
    imports = sorted(set(re.findall(r'^\s+([\w.-]+\.dll)\s*$', run([dumpbin, '/DEPENDENTS', path]), re.M | re.I)))
    if not imports:
        raise ValueError('No import table read: ' + str(path))
    local = {p.name.lower() for folder in (path.parent, root) for p in folder.glob('*.dll')}
    for name in imports:
        lower = name.lower()
        if re.search(r'(?i)(vcruntime\d.*d|msvcp\d.*d|ucrtbased)\.dll$', name):
            raise ValueError('Debug runtime import: ' + name)
        if lower not in local and not lower.startswith(('api-ms-win-', 'ext-ms-win-')) and Path(lower).stem not in SYSTEM_DLLS:
            raise ValueError(f'Unresolved non-system dependency {name} in {path}')
    return dict(file=Path(os.path.relpath(path,root)).as_posix(), bytes=path.stat().st_size, sha256=sha(path), imports=imports)


class Engine:
    def __init__(self, args, cwd, env):
        self.child = subprocess.Popen([str(x) for x in args], cwd=cwd, env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding='utf-8', creationflags=HIDDEN)
        self.lines = queue.Queue()
        self.stderr = []
        def reader():
            for line in self.child.stdout:
                self.lines.put(line)
            self.lines.put(None)
        threading.Thread(target=reader, daemon=True).start()
        threading.Thread(target=lambda: self.stderr.append(self.child.stderr.read()), daemon=True).start()
        self.serial = 0

    def call(self, operation, **args):
        self.serial += 1
        request = dict(schema_version=1, id=str(self.serial), op=operation, **args)
        self.child.stdin.write(json.dumps(request, ensure_ascii=False) + '\n'); self.child.stdin.flush()
        line = self.lines.get(timeout=10)
        if line is None:
            raise RuntimeError('Engine ended before reply: ' + ''.join(self.stderr))
        result = json.loads(line)
        if result.get('id') != request['id'] or not result.get('success'):
            raise AssertionError(result)
        return result['data']

    def task(self, graph):
        self.call('graph.validate', mode='offline', graph=graph)
        task = self.call('tasks.start', mode='offline', graph=graph)['task_id']
        until = time.monotonic() + 15
        while time.monotonic() < until:
            state = self.call('tasks.status', task_id=task)['state']
            if state == 'succeeded':
                result = self.call('tasks.result', task_id=task)
                self.call('tasks.release', task_id=task)
                return result
            if state in ('failed','cancelled'):
                raise AssertionError(self.call('tasks.result', task_id=task))
            time.sleep(.02)
        raise TimeoutError('Engine task timed out')

    def close(self):
        self.child.stdin.close()
        try:
            code = self.child.wait(timeout=10)
            if code: raise RuntimeError(f'Engine exit {code}')
        finally:
            if self.child.poll() is None: self.child.kill(); self.child.wait()


def verify(package, output, dumpbin, plugin):
    if output.exists(): raise FileExistsError('Choose a new verification directory')
    output.mkdir(parents=True)
    moved = output / '中文 空格 便携程序'
    shutil.copytree(package, moved)
    # Includes delay-load imports returned by DUMPBIN /DEPENDENTS.
    pe_files = [p for p in moved.rglob('*') if p.suffix.lower() in ('.dll','.exe')]
    dependencies = [audit_pe(p, moved, dumpbin) for p in pe_files]
    crt = json.loads((moved/'microsoft-crt.json').read_text(encoding='utf-8-sig'))
    for item in crt['files']:
        assert sha(moved/item['name']) == item['sha256']
    with zipfile.ZipFile(moved/'ffmpeg/sources/ffmpeg-audio-sources.zip') as archive:
        assert archive.testzip() is None
        lock = json.loads((moved/'ffmpeg/upstream-lock.json').read_text(encoding='utf-8-sig'))
        for key in ('ffmpeg_source','lame_source'):
            item=lock[key]
            with archive.open('sources/'+item['name']) as source:
                assert hashlib.file_digest(source,'sha256').hexdigest()==item['sha256']
        assert 'scripts/build-ffmpeg-audio.sh' in archive.namelist()
    workspace = output/'独立 工作区'; workspace.mkdir()
    cwd = output/'独立 启动目录'; cwd.mkdir()
    temp = output/'temp'; temp.mkdir()
    system = Path(os.environ['SystemRoot'])
    env = dict(SystemRoot=str(system), WINDIR=str(system), PATH=str(system/'System32'),
        TEMP=str(temp), TMP=str(temp), APPDATA=str(output/'appdata'), LOCALAPPDATA=str(output/'localdata'))
    samples = [round(2000*math.sin(i*.1)) for i in range(4800)]
    original = workspace/'原始.wav'
    with wave.open(str(original),'wb') as audio:
        audio.setparams((1,2,48000,len(samples),'NONE','not compressed')); audio.writeframes(struct.pack('<4800h',*samples))
    original_hash = sha(original)
    ffmpeg=moved/'ffmpeg/bin/ffmpeg.exe'; probe=moved/'ffmpeg/bin/ffprobe.exe'
    formats={'wav':('pcm_s16le','wav'),'flac':('flac','flac'),'mp3':('libmp3lame','mp3'),'m4a':('aac','ipod')}
    for ext,(codec,container) in formats.items():
        dest=workspace/('转换.'+ext)
        run([ffmpeg,'-hide_banner','-nostdin','-loglevel','error','-n','-i',original,'-map','0:a:0','-c:a',codec,'-f',container,dest],cwd=cwd,env=env)
        info=json.loads(run([probe,'-v','error','-show_entries','stream=codec_name,sample_rate,channels','-of','json',dest],cwd=cwd,env=env))
        assert info['streams'][0]['channels']==1 and int(info['streams'][0]['sample_rate'])==48000
    assert sha(original)==original_hash
    engine=Engine([moved/'control-cli.exe','--workspace',workspace],cwd,env)
    try:
        caps=engine.call('capabilities'); assert not caps['policy']['allow_devices']
        graph=dict(schema_version=1,nodes=[dict(id='text',type='text_input',parameters=dict(text='Release IPC'))],connections=[],exports=[dict(name='message',node='text',port='text')])
        assert engine.task(graph)['result']['outputs']['message']['value']=='Release IPC'
    finally: engine.close()
    plugin_name=None
    if plugin:
        trusted=output/'trusted-plugin';shutil.copytree(plugin,trusted)
        for file in trusted.rglob('*.dll'): dependencies.append(audit_pe(file,moved,dumpbin))
        document=dict(schema_version=1,packages=[dict(root=str(trusted),manifest_sha256=sha(trusted/'manifest.json'))])
        snapshot=output/'plugin-snapshot.json';snapshot.write_text(json.dumps(document),encoding='utf-8')
        data=output/'plugin-data';data.mkdir()
        engine=Engine([moved/'control-cli.exe','--workspace',workspace,'--plugin-snapshot',snapshot,'--plugin-snapshot-sha256',sha(snapshot),'--plugin-data',data],cwd,env)
        try:
            plugin_name='org.audiograph.example.gain_v1'
            assert any(n['typeId']==plugin_name for n in engine.call('nodes.list')['nodes'])
            graph=dict(schema_version=1,nodes=[dict(id='input',type='wav_input',parameters=dict(path='原始.wav')),
                dict(id='gain',type=plugin_name,parameters=dict(gain_db=6)),dict(id='output',type='wav_output',parameters=dict(path='插件结果.wav'))],
                connections=[dict(**{'from':dict(node='input',port='audio'),'to':dict(node='gain',port='audio')}),dict(**{'from':dict(node='gain',port='audio'),'to':dict(node='output',port='audio')})])
            engine.task(graph)
            with wave.open(str(workspace/'插件结果.wav'),'rb') as audio:
                values=struct.unpack('<4800h',audio.readframes(4800));assert 1.9<max(values)/max(samples)<2.1
        finally: engine.close()
    report=dict(passed=True,package=str(package),relocated=str(moved),pe_files=dependencies,formats=list(formats),plugin=plugin_name,
        native_window_opened=False,clean_machine_verified=False,limitations=['No Tauri/WebView2 rendering test','Same Windows host with system-only PATH; not a clean VM','User settings and workspaces untouched'])
    (output/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps({k:v for k,v in report.items() if k!='pe_files'},ensure_ascii=False,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for flag in ('package','output','dumpbin'): parser.add_argument('--'+flag,type=Path,required=True)
    parser.add_argument('--plugin',type=Path)
    args=parser.parse_args()
    old_mode=ctypes.windll.kernel32.SetErrorMode(0x0001|0x0002|0x8000) if os.name=='nt' else None
    try: verify(args.package.resolve(),args.output.resolve(),args.dumpbin.resolve(),args.plugin.resolve() if args.plugin else None)
    finally:
        if old_mode is not None: ctypes.windll.kernel32.SetErrorMode(old_mode)
