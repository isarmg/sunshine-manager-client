import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PureWindowsPath
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

spec = importlib.util.spec_from_file_location("client_package", Path(__file__).resolve().parents[1] / "check-client-package.py")
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)
SHA = "a" * 40
VERSION = "0.1.0-rc.1"


class PackageTests(unittest.TestCase):
    def test_windows_powershell_does_not_inherit_pwsh_modules(self):
        with patch.dict(os.environ, {"PSModulePath": "incompatible-pwsh-modules", "KEEP_TEST_VALUE": "kept"}, clear=True):
            self.assertEqual(checker.powershell_environment(), {"KEEP_TEST_VALUE": "kept"})

    @unittest.skipUnless(os.name == "nt", "Windows PowerShell native check")
    def test_windows_security_module_loads(self):
        self.assertEqual(checker.powershell("Import-Module Microsoft.PowerShell.Security; (Get-Command Set-Acl).Name"), "Set-Acl")

    def test_packager_inputs_exist_in_standalone_checkout(self):
        builder_spec = importlib.util.spec_from_file_location("client_builder", Path(__file__).resolve().parents[1] / "package-client.py")
        builder = importlib.util.module_from_spec(builder_spec)
        builder_spec.loader.exec_module(builder)
        for source in builder.COMMON_FILES:
            self.assertTrue((builder.ROOT / source).is_file(), source)
        directory = Mock()
        directory.iterdir.return_value = [PureWindowsPath(name) for name in ['bootstrap.example.json', 'README.md', 'LICENSE']]
        self.assertEqual([p.name for p in builder.package_files(directory)], ['LICENSE', 'README.md', 'bootstrap.example.json'])

    def fixture(self, temporary, windows=False, extra=None, wrong_sha=False, mac_target=None):
        target = mac_target or ("x86_64-pc-windows-msvc" if windows else "x86_64-unknown-linux-gnu")
        name = f"sunshine-client-{VERSION}-{target}"
        names = ["sunshine-client.exe" if windows else "sunshine-client", "README.md", "platform-setup.md", "LICENSE", "bootstrap.example.json"]
        names += ["install-windows.ps1", "uninstall-windows.ps1"] if windows else ["repair-existing.sh", "install-macos.sh", "uninstall-macos.sh", "org.sarmg.sunshine-client.plist"] if mac_target else ["repair-existing.sh", "install-linux.sh", "uninstall-linux.sh", "sunshine-client.service"]
        files = {n: b"fixture" for n in names}
        hashes = {n: hashlib.sha256(b).hexdigest() for n, b in files.items()}
        manifest = {"product": "sunshine-client", "version": VERSION, "source_commit": "b" * 40 if wrong_sha else SHA, "target": target, "protocol": "sunshine-management/1", "authenticode_signed": False, "files": hashes}
        files["manifest.json"] = json.dumps(manifest).encode()
        files["SHA256SUMS"] = "".join(f"{hashlib.sha256(b).hexdigest()}  {n}\n" for n, b in sorted(files.items())).encode()
        entries = [(name + "/" + n, b) for n, b in files.items()]
        if extra is not None:
            entries.append((extra.replace("ROOT", name), b"bad"))
        archive = temporary / (name + (".zip" if windows else ".tar.gz"))
        if windows:
            with zipfile.ZipFile(archive, "w") as out:
                for n, b in entries:
                    out.writestr(n, b)
        else:
            with tarfile.open(archive, "w:gz") as out:
                for n, b in entries:
                    entry = tarfile.TarInfo(n)
                    entry.size = len(b)
                    out.addfile(entry, io.BytesIO(b))
        archive.with_name(archive.name + ".sha256").write_text(f"{checker.digest(archive)}  {archive.name}\n")
        return archive

    def test_both_platform_packages(self):
        for windows in [False, True]:
            with self.subTest(windows=windows), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                archive = self.fixture(root, windows=windows)
                with patch.object(checker, "run", return_value=f"sunshine-client {VERSION} (git {SHA}; sunshine-management/1)"):
                    checker.verify(archive, root, SHA)

    def test_macos_archives_keep_their_actual_architecture(self):
        for target in ["aarch64-apple-darwin"]:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                archive = self.fixture(root, mac_target=target)
                with patch.object(checker, "run", return_value=f"sunshine-client {VERSION} (git {SHA}; sunshine-management/1)"):
                    checker.verify(archive, root, SHA)

    def test_intel_macos_archive_is_not_a_supported_release(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = self.fixture(root, mac_target="x86_64-apple-darwin")
            with patch.object(checker, "run") as executable, self.assertRaises(ValueError):
                checker.verify(archive, root, SHA)
            executable.assert_not_called()

    def test_traversal_extra_and_duplicate_entries_rejected(self):
        for windows in [False, True]:
            for extra in ["ROOT/../escape", "ROOT/unknown", "ROOT/README.md", "/absolute"]:
                with self.subTest(windows=windows, extra=extra), tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    archive = self.fixture(root, windows=windows, extra=extra)
                    with self.assertRaises(ValueError):
                        checker.verify(archive, root, SHA)

    def test_source_mismatch_rejected_before_execution(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = self.fixture(root, wrong_sha=True)
            with patch.object(checker, "run") as executable, self.assertRaises(ValueError):
                checker.verify(archive, root, SHA)
            executable.assert_not_called()

    def test_tampered_archive_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = self.fixture(root)
            with archive.open("ab") as file:
                file.write(b"modified")
            with self.assertRaises(ValueError):
                checker.verify(archive, root, SHA)

    def test_install_test_refuses_user_host(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false"}), self.assertRaises(ValueError):
            checker.install_test(None, None, None)


if __name__ == "__main__":
    unittest.main()
