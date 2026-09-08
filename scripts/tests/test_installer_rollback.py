"""Inject command failures into isolated copies of the actual install scripts.

These exercise rollback control flow and filesystem outcomes on Linux. They do
not replace native service/account/ACL acceptance on the target OS.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SHIM = r'''#!/usr/bin/env python3
import os, sys, json, subprocess
from pathlib import Path
name = Path(sys.argv[0]).name
args = sys.argv[1:]
root = Path(os.environ["FIXTURE_ROOT"])
accounts = root / "accounts"
accounts.mkdir(exist_ok=True)
if name == "id": print("0"); sys.exit(0)
if name == "uname": print("x86_64" if args == ["-m"] else os.environ["FIXTURE_OS"]); sys.exit(0)
if name == "stat":
 p = Path(args[-1]); print(p.stat().st_uid if args[-2] in ("%u",) else oct(p.stat().st_mode & 0o777)[2:]); sys.exit(0)
if name == "getent": sys.exit(0 if (accounts / args[0]).exists() else 2)
if name == "dscl" and args[1] == "-read": sys.exit(0 if (accounts / ("passwd" if args[2].startswith("/Users/") else "group")).exists() else 1)
if name == "dscl" and args[1] == "-search": sys.exit(0)
if name in ("install", "mkdir", "ln", "chown", "dscl", "useradd", "systemctl", "launchctl"):
 counter = root / "counter"
 count = int(counter.read_text()) + 1 if counter.exists() else 1
 counter.write_text(str(count))
 if count == int(os.environ.get("FAIL_STEP", "0")):
  (root / "injected").touch(); sys.exit(91)
if name == "dscl":
 p = accounts / ("passwd" if args[2].startswith("/Users/") else "group")
 if args[1] == "-create": p.touch()
 if args[1] == "-delete": p.unlink(missing_ok=True)
 sys.exit(0)
if name == "useradd":
 (accounts / "passwd").touch(); (accounts / "group").touch(); sys.exit(0)
if name in ("userdel", "groupdel"):
 (accounts / ("passwd" if name == "userdel" else "group")).unlink(missing_ok=True); sys.exit(0)
if name in ("chown", "plutil"): sys.exit(0)
if name in ("launchctl", "systemctl"):
 if any(v in args for v in ("start", "restart", "bootstrap", "kickstart", "--now")): sys.exit(92)
 sys.exit(0)
if name == "install":
 cleaned = []; it = iter(args)
 for arg in it:
  if arg in ("-o", "-g"): next(it)
  else: cleaned.append(arg)
 args = cleaned
sys.exit(subprocess.run(["/usr/bin/" + name] + args).returncode)
'''


@unittest.skipUnless(os.name == "posix" and shutil.which("bash"), "POSIX shell fixture")
class InstallerRollbackTests(unittest.TestCase):
    def fixture(self, os_name, fail=0, existing=False):
        temporary = tempfile.TemporaryDirectory(prefix="client-install-rollback-")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        system = root / "system"
        for path in ["opt", "var/lib", "var/log", "etc/systemd/system", "etc/newsyslog.d",
                     "usr/local/bin", "usr/local/libexec", "Library/LaunchDaemons", "Library/Application Support"]:
            (system / path).mkdir(parents=True, exist_ok=True)
        shim = root / "commands"
        shim.mkdir()
        for command in ["id", "uname", "stat", "getent", "dscl", "useradd", "userdel", "groupdel",
                        "install", "mkdir", "ln", "chown", "plutil", "systemctl", "launchctl"]:
            p = shim / command
            p.write_text(SHIM)
            p.chmod(0o755)
        source = ROOT / "deploy" / ("macos/install-macos.sh" if os_name == "Darwin" else "install-linux.sh")
        contents = source.read_text()
        prefixes = ["/Library", "/usr/local", "/var/log", "/etc/newsyslog.d", "/opt", "/var/lib", "/etc/systemd/system"]
        for prefix in prefixes:
            contents = contents.replace(prefix, str(system) + prefix)
        contents = contents.replace("/usr/bin/plutil", str(shim / "plutil"))
        script = root / "install.sh"
        script.write_text(contents)
        resource = "macos/org.sarmg.sunshine-client.plist" if os_name == "Darwin" else "sunshine-client.service"
        shutil.copy2(ROOT / "deploy" / resource, root / Path(resource).name)
        binary = root / "verified-client"
        binary.write_text("#!/bin/sh\necho 'sunshine-client fixture'\n")
        binary.chmod(0o755)
        targets = (["Library/Application Support/sunshine-client", "Library/LaunchDaemons/org.sarmg.sunshine-client.plist",
                    "usr/local/libexec/sunshine-client", "usr/local/bin/sunshine-client", "var/log/sunshine-client.log",
                    "etc/newsyslog.d/sunshine-client.conf"] if os_name == "Darwin" else
                   ["opt/sunshine-client", "var/lib/sunshine-client", "etc/systemd/system/sunshine-client.service", "usr/local/bin/sunshine-client"])
        if existing:
            (system / targets[0]).mkdir()
            (system / targets[0] / "identity").write_text("preserve-existing-identity")
        env = dict(os.environ, PATH=str(shim) + os.pathsep + os.environ["PATH"], FIXTURE_ROOT=str(root), FIXTURE_OS=os_name, FAIL_STEP=str(fail))
        result = subprocess.run(["bash" if os_name == "Linux" else "sh", str(script), str(binary)], env=env, capture_output=True, text=True, timeout=20)
        return result, root, system, targets

    def test_each_install_failure_rolls_back_only_new_artifacts(self):
        for os_name in ["Linux", "Darwin"]:
            success, root, system, targets = self.fixture(os_name)
            self.assertEqual(success.returncode, 0, success.stderr)
            count = int((root / "counter").read_text())
            for fail in range(1, count + 1):
                with self.subTest(os=os_name, step=fail):
                    result, fixture, system, targets = self.fixture(os_name, fail)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertTrue((fixture / "injected").exists(), result.stderr)
                    for target in targets:
                        self.assertFalse(os.path.lexists(system / target), f"retained {target}: {result.stderr}")
                    self.assertEqual(list((fixture / "accounts").iterdir()), [], result.stderr)

    def test_existing_identity_is_never_removed_or_repaired(self):
        for os_name in ["Linux", "Darwin"]:
            result, root, system, targets = self.fixture(os_name, existing=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual((system / targets[0] / "identity").read_text(), "preserve-existing-identity")
            self.assertFalse((root / "counter").exists())
