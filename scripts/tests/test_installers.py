import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]


class InstallerTests(unittest.TestCase):
    def test_windows_service_is_separate_and_not_started_before_pairing(self):
        tree = ET.parse(ROOT / 'packaging/windows/Package.wxs')
        ns = {'w': 'http://wixtoolset.org/schemas/v4/wxs'}
        service = tree.find('.//w:ServiceInstall', ns)
        self.assertEqual(service.get('Name'), 'SunshineClient')
        self.assertEqual(service.get('Start'), 'auto')
        self.assertIn('service --state', service.get('Arguments'))
        self.assertIsNone(tree.find('.//w:ServiceControl', ns).get('Start'))
        self.assertTrue(tree.findall('.//w:RegistryValue', ns))

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
            deb = directory / 'sunshine-client_0.1.0~rc1_amd64.deb'
            self.assertEqual(subprocess.check_output(['dpkg-deb', '-f', str(deb), 'Architecture'], text=True).strip(), 'amd64')
            subprocess.run(['dpkg-deb', '-x', str(deb), str(directory / 'extracted')], check=True)
            setup = directory / 'extracted/usr/bin/sunshine-client-setup'
            self.assertEqual(setup.stat().st_mode & 0o777, 0o755)
            self.assertIn("getpass.getpass", setup.read_text())
            subprocess.run(['dpkg-deb', '-e', str(deb), str(directory / 'control')], check=True)
            for script in ('preinst', 'postinst', 'prerm', 'postrm'):
                subprocess.run(['sh', '-n', str(directory / 'control' / script)], check=True)


if __name__ == '__main__':
    unittest.main()
