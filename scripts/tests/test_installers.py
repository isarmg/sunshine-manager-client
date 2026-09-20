import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]


class InstallerTests(unittest.TestCase):
    def test_windows_service_install_does_not_launch_interactive_setup(self):
        tree = ET.parse(ROOT / 'packaging/windows/Package.wxs')
        ns = {
            'w': 'http://wixtoolset.org/schemas/v4/wxs',
            'ui': 'http://wixtoolset.org/schemas/v4/wxs/ui',
        }
        service = tree.find('.//w:ServiceInstall', ns)
        self.assertEqual(service.get('Name'), 'SunshineClient')
        self.assertEqual(service.get('Start'), 'demand')
        self.assertIn('service --state', service.get('Arguments'))
        self.assertIsNone(tree.find('.//w:ServiceControl', ns).get('Start'))
        ui = tree.find('.//ui:WixUI', ns)
        self.assertEqual(ui.get('Id'), 'WixUI_FeatureTree')
        self.assertEqual(ui.get('InstallDirectory'), 'INSTALLFOLDER')
        install_location = tree.find('.//w:RegistryValue[@Name="InstallLocation"]', ns)
        self.assertIsNotNone(install_location)
        self.assertEqual(install_location.get('Value'), '[INSTALLFOLDER]')
        features = {feature.get('Id'): feature for feature in tree.findall('.//w:Feature', ns)}
        self.assertEqual(features['PreserveConfiguration'].get('AllowAbsent'), 'yes')
        self.assertEqual(features['PrepareSetup'].get('AllowAbsent'), 'yes')
        self.assertEqual(features['PreserveData'].get('AllowAbsent'), 'yes')
        actions = {action.get('Id'): action for action in tree.findall('.//w:CustomAction', ns)}
        self.assertEqual(actions['ResetOldConfiguration'].get('ExeCommand'), 'installer reset-configuration')
        self.assertEqual(actions['ResetOldData'].get('ExeCommand'), 'installer reset-data')
        self.assertEqual(actions['PrepareSetupState'].get('ExeCommand'), 'installer prepare-setup')
        self.assertEqual(actions['PrepareSetupState'].get('Execute'), 'commit')
        self.assertEqual(actions['PrepareSetupState'].get('Return'), 'check')
        self.assertEqual(actions['PrepareSetupState'].get('FileRef'), 'ClientExe')
        self.assertEqual(actions['PrepareSetupState'].get('Impersonate'), 'no')
        self.assertEqual(actions['ResetOldConfiguration'].get('Return'), 'check')
        self.assertEqual(actions['ResetOldData'].get('Return'), 'check')
        sequence = {
            action.get('Action'): action.get('Condition')
            for action in tree.findall('.//w:InstallExecuteSequence/w:Custom', ns)
        }
        self.assertIn('&PreserveConfiguration <> 3', sequence['ResetOldConfiguration'])
        self.assertIn('&PreserveData <> 3', sequence['ResetOldData'])
        self.assertIn('&PrepareSetup = 3', sequence['PrepareSetupState'])
        self.assertEqual(
            next(custom for custom in tree.findall('.//w:InstallExecuteSequence/w:Custom', ns)
                 if custom.get('Action') == 'PrepareSetupState').get('After'),
            'ResetOldData',
        )
        path_entry = tree.find('.//w:Environment[@Name="PATH"]', ns)
        self.assertIsNotNone(path_entry)
        self.assertEqual(path_entry.get('Value'), '[INSTALLFOLDER]')
        self.assertEqual(path_entry.get('Action'), 'set')
        self.assertEqual(path_entry.get('Part'), 'last')
        self.assertEqual(path_entry.get('Permanent'), 'no')
        self.assertEqual(path_entry.get('System'), 'yes')
        self.assertIsNone(tree.find('.//w:CustomAction[@Id="LaunchInteractiveSetup"]', ns))
        self.assertIsNone(tree.find('.//w:InstallExecuteSequence/w:Custom[@Action="LaunchInteractiveSetup"]', ns))
        self.assertNotIn('TrayExe', (ROOT / 'packaging/windows/Package.wxs').read_text())

    def test_deb_build_and_private_setup_contract(self):
        import shutil
        if not shutil.which('dpkg-deb'):
            self.skipTest('Debian package tools unavailable')
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            binary = directory / 'sunshine-client'
            binary.write_bytes(b'package-layout-fixture')
            binary.chmod(0o755)
            subprocess.run(['python3', str(ROOT / 'scripts/build-linux-installer.py'), '--binary', str(binary), '--output', tmp, '--version', '0.1.0-rc.1'], check=True, stdout=subprocess.DEVNULL)
            deb = directory / 'sunshine-client_0.1.0-rc.1_amd64.deb'
            self.assertEqual(subprocess.check_output(['dpkg-deb', '-f', str(deb), 'Version'], text=True).strip(), '0.1.0~rc1')
            self.assertEqual(subprocess.check_output(['dpkg-deb', '-f', str(deb), 'Architecture'], text=True).strip(), 'amd64')
            subprocess.run(['dpkg-deb', '-x', str(deb), str(directory / 'extracted')], check=True)
            self.assertFalse((directory / 'extracted/usr/bin/sunshine-client-setup').exists())
            subprocess.run(['dpkg-deb', '-e', str(deb), str(directory / 'control')], check=True)
            for script in ('preinst', 'postinst', 'prerm', 'postrm'):
                subprocess.run(['sh', '-n', str(directory / 'control' / script)], check=True)

    def test_macos_pkg_is_native_and_keeps_setup_outside_the_install_transaction(self):
        builder = ROOT / 'packaging/macos/build-pkg.sh'
        preinstall = ROOT / 'packaging/macos/scripts/preinstall'
        postinstall = ROOT / 'packaging/macos/scripts/postinstall'
        self.assertTrue(builder.is_file())
        self.assertTrue(preinstall.is_file())
        self.assertTrue(postinstall.is_file())
        self.assertIn('pkgbuild', builder.read_text())
        postinstall_text = postinstall.read_text()
        self.assertNotIn('setup --interactive', postinstall_text)
        self.assertNotIn('"$link" setup', postinstall_text)
        self.assertIn('Configuration is pending', postinstall_text)
        self.assertIn('/private/etc/newsyslog.d', postinstall_text)
        self.assertIn('/private/var/run', postinstall_text)
        self.assertNotIn('install-macos.sh', builder.read_text())
        for script in (builder, preinstall, postinstall):
            subprocess.run(['sh', '-n', str(script)], check=True)


if __name__ == '__main__':
    unittest.main()
