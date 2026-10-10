import importlib.util
import io
from pathlib import Path
import subprocess
import tarfile
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
        self.assertEqual(service.get('Name'), 'Xscc')
        self.assertEqual(service.get('Start'), 'auto')
        self.assertIn('service --state', service.get('Arguments'))
        self.assertIsNone(tree.find('.//w:ServiceControl', ns).get('Start'))
        ui = tree.find('.//ui:WixUI', ns)
        self.assertEqual(ui.get('Id'), 'WixUI_InstallDir')
        self.assertEqual(ui.get('InstallDirectory'), 'INSTALLFOLDER')
        install_location = tree.find('.//w:RegistryValue[@Name="InstallLocation"]', ns)
        self.assertIsNotNone(install_location)
        self.assertEqual(install_location.get('Value'), '[INSTALLFOLDER]')
        features = tree.findall('.//w:Feature', ns)
        self.assertEqual(len(features), 1)
        self.assertEqual(features[0].get('Id'), 'Complete')
        self.assertEqual(features[0].get('AllowAbsent'), 'no')
        components = {component.get('Id') for component in tree.findall('.//w:Component', ns)}
        references = {reference.get('Id') for reference in features[0].findall('w:ComponentRef', ns)}
        self.assertEqual(references, components)
        actions = {action.get('Id'): action for action in tree.findall('.//w:CustomAction', ns)}
        self.assertNotIn('ResetOldConfiguration', actions)
        self.assertNotIn('ResetOldData', actions)
        self.assertEqual(actions['PrepareSetupState'].get('ExeCommand'), 'installer prepare-setup')
        self.assertEqual(actions['PrepareSetupState'].get('Execute'), 'commit')
        self.assertEqual(actions['PrepareSetupState'].get('Return'), 'check')
        self.assertEqual(actions['PrepareSetupState'].get('FileRef'), 'ClientExe')
        self.assertEqual(actions['PrepareSetupState'].get('Impersonate'), 'no')
        sequence = {
            action.get('Action'): action.get('Condition')
            for action in tree.findall('.//w:InstallExecuteSequence/w:Custom', ns)
        }
        self.assertEqual(sequence, {'PrepareSetupState': 'NOT REMOVE~="ALL"'})
        self.assertEqual(
            next(custom for custom in tree.findall('.//w:InstallExecuteSequence/w:Custom', ns)
                 if custom.get('Action') == 'PrepareSetupState').get('After'),
            'InstallFiles',
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
        self.assertFalse(tree.findall('.//w:RegistryValue[@Key="Software\\Microsoft\\Windows\\CurrentVersion\\Run"]', ns))
        self.assertFalse(tree.findall('.//w:StartupTask', ns))

    def test_deb_build_and_private_setup_contract(self):
        import shutil
        if not shutil.which('dpkg-deb'):
            self.skipTest('Debian package tools unavailable')
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            binary = directory / 'xscc'
            binary.write_bytes(b'package-layout-fixture')
            binary.chmod(0o755)
            subprocess.run(['python3', str(ROOT / 'scripts/build-linux-installer.py'), '--binary', str(binary), '--output', tmp, '--version', '0.1.0-rc.1'], check=True, stdout=subprocess.DEVNULL, umask=0o077)
            deb = directory / 'xscc_0.1.0-rc.1_amd64.deb'
            self.assertEqual(subprocess.check_output(['dpkg-deb', '-f', str(deb), 'Version'], text=True).strip(), '0.1.0~rc1')
            self.assertEqual(subprocess.check_output(['dpkg-deb', '-f', str(deb), 'Architecture'], text=True).strip(), 'amd64')
            data_tar = subprocess.check_output(['dpkg-deb', '--fsys-tarfile', str(deb)])
            with tarfile.open(fileobj=io.BytesIO(data_tar)) as payload:
                directories = [entry for entry in payload if entry.isdir()]
            self.assertTrue(any(entry.name.rstrip('/') == '.' for entry in directories))
            for entry in directories:
                with self.subTest(directory=entry.name):
                    self.assertEqual(entry.mode, 0o755)
                    self.assertEqual((entry.uid, entry.gid), (0, 0))
            subprocess.run(['dpkg-deb', '-x', str(deb), str(directory / 'extracted')], check=True)
            self.assertFalse((directory / 'extracted/usr/bin/xscc-setup').exists())
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
        self.assertIn('provisioning="$state/provisioning"', postinstall_text)
        self.assertIn("find \"$provisioning\" -type l -print -quit", postinstall_text)
        self.assertIn('[ "$owner" = 0 ]', postinstall_text)
        self.assertIn("[ \"$mode\" = 700 ]", postinstall_text)
        self.assertIn("[ \"$mode\" = 600 ] && [ \"$links\" = 1 ]", postinstall_text)
        self.assertIn('chown "$account:$group"', postinstall_text)
        preinstall_text = preinstall.read_text()
        self.assertNotIn('stale launchd transaction marker exists', preinstall_text)
        self.assertIn('Unsafe launchd transaction marker exists', preinstall_text)
        self.assertIn('if [ ! -e "$marker" ]; then', preinstall_text)
        self.assertNotIn('install-macos.sh', builder.read_text())
        for script in (builder, preinstall, postinstall):
            subprocess.run(['sh', '-n', str(script)], check=True)


if __name__ == '__main__':
    unittest.main()
