"""Exercise actual generated DEB scripts and the Linux installer in isolated paths.

System commands are simulated, so these prove installation transactions and
startup intent; disposable GitHub runners still exercise real systemd.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SHIM = r'''#!/usr/bin/env python3
import json, os, shutil, subprocess, sys
from pathlib import Path
name = Path(sys.argv[0]).name
args = sys.argv[1:]
root = Path(os.environ['FIXTURE_ROOT'])
state = Path(os.environ['FIXTURE_STATE'])
accounts = root / 'account'
if name == 'id':
 print(1001 if args == ['-u', 'sunshine-client'] else 0); sys.exit(0)
if name == 'uname':
 print('x86_64' if args == ['-m'] else 'Linux'); sys.exit(0)
if name == 'stat':
 path = Path(args[-1]); pattern = args[-2]
 if pattern == '%u': print(1001 if path == state else 0)
 elif pattern == '%a': print(oct(path.stat().st_mode & 0o777)[2:])
 else: sys.exit(2)
 sys.exit(0)
if name == 'getent':
 if not accounts.exists(): sys.exit(2)
 if args[0] == 'passwd': print(f'sunshine-client:x:1001:1001::{state}:/usr/sbin/nologin')
 else: print('sunshine-client:x:1001:')
 sys.exit(0)
if name == 'useradd': accounts.touch(); sys.exit(0)
if name in ('userdel', 'groupdel'): accounts.unlink(missing_ok=True); sys.exit(0)
if name == 'chown': sys.exit(0)
if name == 'systemctl':
 service_file = root / 'service.json'
 service = json.loads(service_file.read_text()) if service_file.exists() else {'enabled': False, 'active': False}
 with (root / 'commands.jsonl').open('a') as log: log.write(json.dumps(args) + '\n')
 action = args[0]
 if action == 'is-enabled': sys.exit(0 if service['enabled'] else 1)
 if action == 'is-active': sys.exit(0 if service['active'] else 3)
 link = root / 'system/etc/systemd/system/multi-user.target.wants/sunshine-client.service'
 if action == 'enable':
  service['enabled'] = True
  link.parent.mkdir(exist_ok=True)
  unit = root / 'system/etc/systemd/system/sunshine-client.service'
  if not unit.exists(): unit = root / 'system/usr/lib/systemd/system/sunshine-client.service'
  if '--force' in args: link.unlink(missing_ok=True)
  if not link.is_symlink(): link.symlink_to(unit)
 if action == 'disable':
  service['enabled'] = False
  link.unlink(missing_ok=True)
  if '--now' in args: service['active'] = False
 if action == 'stop': service['active'] = False
 service_file.write_text(json.dumps(service))
 if action == os.environ.get('FAIL_ACTION') and not (root / 'injected').exists():
  (root / 'injected').touch(); sys.exit(91)
 if action == 'start':
  service['active'] = (state / 'provisioning/identity.json').is_file()
  service_file.write_text(json.dumps(service))
 sys.exit(0)
if name == 'install':
 cleaned = []; iterator = iter(args)
 for arg in iterator:
  if arg in ('-o', '-g'): next(iterator)
  else: cleaned.append(arg)
 args = cleaned
real = shutil.which(name, path='/usr/bin:/bin:/usr/sbin:/sbin')
if not real: sys.exit(127)
sys.exit(subprocess.run([real, *args]).returncode)
'''


@unittest.skipUnless(os.name == 'posix' and shutil.which('dpkg-deb'), 'Linux package tooling')
class LinuxInstallerDefaultsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.package = tempfile.TemporaryDirectory(prefix='client-linux-defaults-deb-')
        cls.addClassCleanup(cls.package.cleanup)
        directory = Path(cls.package.name)
        binary = directory / 'verified-client'
        binary.write_text("#!/bin/sh\nprintf 'sunshine-client fixture\\n'\n")
        binary.chmod(0o755)
        subprocess.run([
            'python3', str(ROOT / 'scripts/build-linux-installer.py'), '--binary', str(binary),
            '--output', str(directory), '--version', '0.3.2',
        ], check=True, capture_output=True)
        deb = directory / 'sunshine-client_0.3.2_amd64.deb'
        cls.control = directory / 'control'
        cls.extracted = directory / 'extracted'
        subprocess.run(['dpkg-deb', '-e', str(deb), str(cls.control)], check=True)
        subprocess.run(['dpkg-deb', '-x', str(deb), str(cls.extracted)], check=True)
        for name in ('preinst', 'postinst', 'prerm', 'postrm'):
            subprocess.run(['sh', '-n', str(cls.control / name)], check=True)

    def fixture(self, enabled=False, paired=False, manual=False):
        temporary = tempfile.TemporaryDirectory(prefix='client-linux-defaults-')
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        system = root / 'system'
        for directory in ('opt', 'var/lib', 'etc/systemd/system', 'usr/local/bin', 'usr/lib/systemd/system', 'run'):
            (system / directory).mkdir(parents=True, exist_ok=True)
        state = system / 'var/lib/sunshine-client'
        state.mkdir(mode=0o700)
        (root / 'account').touch()
        if paired:
            (state / 'provisioning').mkdir(mode=0o700)
            (state / 'provisioning/identity.json').write_text('preserved-fixture-identity')
            (state / 'journal').mkdir(mode=0o700)
            (state / 'journal/operation.json').write_text('preserved-fixture-receipt')
        service = {'enabled': enabled, 'active': False}
        (root / 'service.json').write_text(json.dumps(service))
        shim = root / 'commands'
        shim.mkdir()
        for command in ('id', 'uname', 'stat', 'getent', 'useradd', 'userdel', 'groupdel', 'chown', 'systemctl', 'install'):
            path = shim / command
            path.write_text(SHIM)
            path.chmod(0o755)
        for script in ('preinst', 'postinst', 'prerm', 'postrm'):
            (root / script).write_text(self.redirect((self.control / script).read_text(), system))
        unit = self.extracted / 'usr/lib/systemd/system/sunshine-client.service'
        shutil.copy2(unit, system / 'usr/lib/systemd/system/sunshine-client.service')
        if manual:
            (system / 'etc/systemd/system/sunshine-client.service').write_text(self.redirect(unit.read_text(), system))
        if enabled:
            link = system / 'etc/systemd/system/multi-user.target.wants/sunshine-client.service'
            link.parent.mkdir()
            link.symlink_to(system / ('etc/systemd/system/sunshine-client.service' if manual else 'usr/lib/systemd/system/sunshine-client.service'))
        env = dict(os.environ, PATH=str(shim) + os.pathsep + os.environ['PATH'], FIXTURE_ROOT=str(root), FIXTURE_STATE=str(state))
        return root, system, state, env

    @staticmethod
    def redirect(contents, system):
        for prefix in ('/opt', '/var/lib', '/etc/systemd/system', '/usr/local/bin', '/usr/lib/systemd/system', '/run'):
            contents = contents.replace(prefix, str(system) + prefix)
        return contents

    def invoke(self, root, env, script, *arguments):
        return subprocess.run(['sh', str(root / script), *arguments], env=env, input='', capture_output=True, text=True, timeout=20)

    @staticmethod
    def commands(root):
        path = root / 'commands.jsonl'
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    @staticmethod
    def service(root):
        return json.loads((root / 'service.json').read_text())

    def test_fresh_deb_enables_systemd_without_starting_or_claiming_online(self):
        root, system, state, env = self.fixture()
        result = self.invoke(root, env, 'postinst', 'configure')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.service(root), {'enabled': True, 'active': False})
        actions = [command[0] for command in self.commands(root)]
        self.assertLess(actions.index('daemon-reload'), actions.index('enable'))
        self.assertNotIn('start', actions)
        self.assertNotIn('online', result.stdout.lower())
        self.assertFalse((state / 'provisioning/identity.json').exists())
        unit = (system / 'usr/lib/systemd/system/sunshine-client.service').read_text()
        self.assertIn('ConditionPathExists=/var/lib/sunshine-client/provisioning/identity.json\n', unit)
        self.assertIn('WantedBy=multi-user.target\n', unit)

    def test_deb_upgrade_retains_disabled_policy_and_restores_running_paired_service(self):
        root, system, state, env = self.fixture(enabled=False, paired=True)
        (root / 'service.json').write_text(json.dumps({'enabled': False, 'active': True}))
        identity = (state / 'provisioning/identity.json').read_bytes()
        receipt = (state / 'journal/operation.json').read_bytes()
        result = self.invoke(root, env, 'prerm', 'upgrade')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.service(root)['active'])
        env['FAIL_ACTION'] = 'start'
        failed = self.invoke(root, env, 'postinst', 'configure', '0.3.1')
        self.assertNotEqual(failed.returncode, 0)
        self.assertTrue((system / 'run/sunshine-client-package-was-active').is_file())
        repaired = self.invoke(root, env, 'postinst', 'configure', '0.3.1')
        self.assertEqual(repaired.returncode, 0, repaired.stderr)
        self.assertEqual(self.service(root), {'enabled': False, 'active': True})
        self.assertFalse((system / 'run/sunshine-client-package-was-active').exists())
        self.assertNotIn('enable', [command[0] for command in self.commands(root)])
        self.assertEqual((state / 'provisioning/identity.json').read_bytes(), identity)
        self.assertEqual((state / 'journal/operation.json').read_bytes(), receipt)

    def test_import_manual_install_preserves_no_and_recovers_after_configure_failure(self):
        root, system, state, env = self.fixture(enabled=False, paired=True, manual=True)
        result = self.invoke(root, env, 'preinst', 'install')
        self.assertEqual(result.returncode, 0, result.stderr)
        policy = system / 'etc/systemd/system/.sunshine-client-package-startup-policy'
        self.assertEqual(policy.read_text(), 'disabled\n')
        env['FAIL_ACTION'] = 'disable'
        failed = self.invoke(root, env, 'postinst', 'configure')
        self.assertNotEqual(failed.returncode, 0)
        self.assertTrue(policy.is_file(), 'failed migration must retain the original choice for a later retry')
        repaired = self.invoke(root, env, 'postinst', 'configure')
        self.assertEqual(repaired.returncode, 0, repaired.stderr)
        self.assertEqual(self.service(root), {'enabled': False, 'active': False})
        self.assertFalse(policy.exists())
        self.assertNotIn('enable', [command[0] for command in self.commands(root)])
        self.assertEqual((state / 'provisioning/identity.json').read_text(), 'preserved-fixture-identity')

    def test_import_manual_install_preserves_yes_and_refuses_redirected_policy(self):
        root, system, state, env = self.fixture(enabled=True, manual=True)
        self.assertEqual(self.invoke(root, env, 'preinst', 'install').returncode, 0)
        self.assertEqual(self.invoke(root, env, 'postinst', 'configure').returncode, 0)
        self.assertTrue(self.service(root)['enabled'])
        self.assertFalse((system / 'etc/systemd/system/sunshine-client.service').exists())
        self.assertEqual((system / 'etc/systemd/system/multi-user.target.wants/sunshine-client.service').resolve(), system / 'usr/lib/systemd/system/sunshine-client.service')
        root, system, state, env = self.fixture(manual=True)
        original = root / 'preserved-file'
        original.write_text('unchanged')
        (system / 'etc/systemd/system/.sunshine-client-package-startup-policy').symlink_to(original)
        rejected = self.invoke(root, env, 'preinst', 'install')
        self.assertEqual(rejected.returncode, 8)
        self.assertEqual(original.read_text(), 'unchanged')
        self.assertEqual(self.service(root), {'enabled': False, 'active': False})

    def test_script_install_rolls_back_partial_enable_links(self):
        for fail in (False, True):
            with self.subTest(fail_enable=fail):
                root, system, state, env = self.fixture()
                state.rmdir()
                (root / 'account').unlink()
                (system / 'usr/lib/systemd/system/sunshine-client.service').unlink()
                script = root / 'install.sh'
                script.write_text(self.redirect((ROOT / 'deploy/install-linux.sh').read_text(), system))
                shutil.copy2(ROOT / 'deploy/sunshine-client.service', root / 'sunshine-client.service')
                binary = root / 'verified-client'
                binary.write_text("#!/bin/sh\nprintf 'sunshine-client fixture\\n'\n")
                binary.chmod(0o755)
                if fail: env['FAIL_ACTION'] = 'enable'
                result = subprocess.run(['bash', str(script), str(binary)], env=env, input='', capture_output=True, text=True, timeout=20)
                if fail:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(self.service(root), {'enabled': False, 'active': False})
                    self.assertFalse((root / 'account').exists())
                    for target in ('opt/sunshine-client', 'var/lib/sunshine-client', 'etc/systemd/system/sunshine-client.service', 'usr/local/bin/sunshine-client', 'etc/systemd/system/multi-user.target.wants/sunshine-client.service'):
                        self.assertFalse(os.path.lexists(system / target), target)
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(self.service(root), {'enabled': True, 'active': False})
                    self.assertIn('unpaired service remains stopped', result.stdout)
                actions = [command[0] for command in self.commands(root)]
                self.assertNotIn('start', actions)
                self.assertLess(actions.index('daemon-reload'), actions.index('enable'))

    def test_script_install_does_not_treat_an_existing_vendor_unit_as_fresh(self):
        root, system, state, env = self.fixture()
        state.rmdir()
        (root / 'account').unlink()
        script = root / 'install.sh'
        script.write_text(self.redirect((ROOT / 'deploy/install-linux.sh').read_text(), system))
        (root / 'repair-existing.sh').write_text("#!/bin/bash\nprintf 'existing-install-repair\\n'\nexit 77\n")
        binary = root / 'verified-client'
        binary.write_text("#!/bin/sh\nprintf 'sunshine-client fixture\\n'\n")
        binary.chmod(0o755)
        result = subprocess.run(['bash', str(script), str(binary)], env=env, input='', capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 77, result.stderr)
        self.assertIn('existing-install-repair', result.stdout)
        self.assertEqual(self.commands(root), [])
        self.assertFalse(state.exists())
        self.assertFalse((system / 'opt/sunshine-client').exists())


if __name__ == '__main__':
    unittest.main()
