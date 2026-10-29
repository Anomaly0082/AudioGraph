"""Pure policy regressions; no NSIS process or registry/desktop operations."""
import importlib.util
from pathlib import Path
import sys
import unittest
sys.dont_write_bytecode=True
spec=importlib.util.spec_from_file_location('installer_check',Path(__file__).with_name('verify-desktop-installer.py'))
checker=importlib.util.module_from_spec(spec);spec.loader.exec_module(checker)


class PolicyTests(unittest.TestCase):
    def setUp(self):
        self.script=r'''
!define BUNDLEID "com.audioprocess.prototype"
!define INSTALLMODE "currentUser"
!define INSTALLWEBVIEW2MODE "embedBootstrapper"
!define MAINBINARYNAME "app"
!define MUI_FINISHPAGE_RUN_NOTCHECKED
!insertmacro AudioGraphRequireClosed
Section WebView2
InitPluginsDir
ReadRegStr $4 HKCU "key" "pv"
ReadRegStr $4 HKLM "key" "pv"
${If} $4 == "0.0.0.0"
File "/oname=$PLUGINSDIR\MicrosoftEdgeWebview2Setup.exe" "bootstrap.exe"
ExecWait '"$6" ${WEBVIEW2INSTALLERARGS} /install' $1
${EndIf}
SectionEnd
!insertmacro AudioGraphRequireClosed
Section Uninstall
!insertmacro AudioGraphRequireClosed
Delete "$INSTDIR\${MAINBINARYNAME}.exe"
Delete "$INSTDIR\ffmpeg\NOTICE.txt"
Delete "$INSTDIR\uninstall.exe"
RMDir "$INSTDIR\ffmpeg"
RMDir "$INSTDIR"
SectionEnd
'''
        self.expected={'app.exe','ffmpeg/NOTICE.txt'}

    def test_owned_files_only_and_user_sentinels_preserved(self):
        self.assertEqual(set(checker.script_policy(self.script,self.expected)),self.expected|{'uninstall.exe'})

    def test_reject_recursive_or_user_data_deletion(self):
        for bad in ('RMDir /r "$INSTDIR"','Delete "$APPDATA\\config.json"','Var DeleteAppDataCheckbox'):
            with self.subTest(bad=bad),self.assertRaises(AssertionError):checker.script_policy(self.script+'\n'+bad,self.expected)

    def test_reject_unowned_workspace_file_and_dynamic_delete(self):
        for path in ('workspace\\input.wav','$OldMainBinaryName'):
            bad=self.script.replace('Section Uninstall','Section Uninstall\nDelete "$INSTDIR\\'+path+'"')
            with self.subTest(path=path),self.assertRaises(AssertionError):checker.script_policy(bad,self.expected)

    def test_reject_process_kill_or_missing_early_guard(self):
        for bad in (self.script+'\nnsis_tauri_utils::KillProcess "app.exe"', self.script.replace('!insertmacro AudioGraphRequireClosed','',1)):
            with self.assertRaises(AssertionError):checker.script_policy(bad,self.expected)

    def test_update_must_not_skip_webview_check(self):
        bad=self.script.replace('Section WebView2','Section WebView2\n${If} $UpdateMode <> 1')
        with self.assertRaises(AssertionError):checker.script_policy(bad,self.expected)

    def test_archive_paths_cannot_escape_or_use_wildcards(self):
        for bad in ('../user.txt','C:\\user.txt','\\\\server\\file','source:ads','*','?'):
            with self.subTest(bad=bad),self.assertRaises(ValueError):checker.relative(bad)
        self.assertEqual(checker.relative(r'ffmpeg\sources\code.zip'),'ffmpeg/sources/code.zip')


if __name__=='__main__':unittest.main()
