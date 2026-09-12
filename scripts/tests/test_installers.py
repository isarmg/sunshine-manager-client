import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]


class InstallerTests(unittest.TestCase):
    def test_windows_service_and_first_run_setup_contract(self):
        tree = ET.parse(ROOT / 'packaging/windows/Package.wxs')
        ns = {'w': 'http://wixtoolset.org/schemas/v4/wxs'}
        service = tree.find('.//w:ServiceInstall', ns)
        self.assertEqual(service.get('Name'), 'SunshineClient')
        self.assertEqual(service.get('Start'), 'demand')
        self.assertIn('service --state', service.get('Arguments'))
        self.assertIsNone(tree.find('.//w:ServiceControl', ns).get('Start'))
        self.assertFalse(tree.findall('.//w:RegistryValue', ns))
        setup = tree.find('.//w:CustomAction[@Id="LaunchInteractiveSetup"]', ns)
        self.assertIsNotNone(setup)
        self.assertEqual(setup.get('FileRef'), 'ClientExe')
        self.assertEqual(setup.get('ExeCommand'), 'setup --interactive')
        self.assertEqual(setup.get('Execute'), 'immediate')
        self.assertEqual(setup.get('Impersonate'), 'yes')
        self.assertEqual(setup.get('Return'), 'ignore')
        sequence = tree.find('.//w:InstallExecuteSequence/w:Custom[@Action="LaunchInteractiveSetup"]', ns)
        self.assertIsNotNone(sequence)
        self.assertEqual(sequence.get('After'), 'InstallFinalize')
        self.assertEqual(sequence.get('Condition'), 'NOT Installed AND NOT REMOVE~="ALL" AND UILevel >= 4')
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

    def test_macos_pkg_is_the_native_installer_and_runs_setup_after_install(self):
        builder = ROOT / 'packaging/macos/build-pkg.sh'
        preinstall = ROOT / 'packaging/macos/scripts/preinstall'
        postinstall = ROOT / 'packaging/macos/scripts/postinstall'
        self.assertTrue(builder.is_file())
        self.assertTrue(preinstall.is_file())
        self.assertTrue(postinstall.is_file())
        self.assertIn('pkgbuild', builder.read_text())
        postinstall_text = postinstall.read_text()
        self.assertIn('setup --interactive', postinstall_text)
        self.assertIn('/private/etc/newsyslog.d', postinstall_text)
        self.assertIn('/private/var/run', postinstall_text)
        self.assertNotIn('install-macos.sh', builder.read_text())
        for script in (builder, preinstall, postinstall):
            subprocess.run(['sh', '-n', str(script)], check=True)


if __name__ == '__main__':
    unittest.main()
