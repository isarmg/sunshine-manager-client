#!/usr/bin/env python3
"""Independently unpack/verify an Client package; native service tests only on disposable CI."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import tarfile
import tempfile
import zipfile


def run(*args):
    env = powershell_environment() if args[0] == "powershell.exe" else None
    return subprocess.check_output(args, text=True, env=env).strip()


def powershell_environment():
    # A pwsh-hosted runner must not inject PowerShell 7 modules into Windows
    # PowerShell 5.1. Let the child initialize its own standard module paths.
    return {key: value for key, value in os.environ.items() if key.lower() != "psmodulepath"}


def powershell(script):
    return run("powershell.exe", "-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; " + script)


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def verify(archive, destination, sha):
    windows = archive.suffix == ".zip"
    suffix = ".zip" if windows else ".tar.gz"
    name = archive.name.removesuffix(suffix)
    match = re.fullmatch(r"sunshine-client-[0-9]+\.[0-9]+\.[0-9]+(?:-rc\.[0-9]+)?-(?P<target>x86_64-pc-windows-msvc|x86_64-unknown-linux-gnu|(?:x86_64|aarch64)-apple-darwin)", name)
    if match is None:
        raise ValueError("unexpected archive name")
    target = match["target"]
    macos = target.endswith("-apple-darwin")
    if windows != (target == "x86_64-pc-windows-msvc"):
        raise ValueError("archive format does not match target")
    checksum = archive.with_name(archive.name + ".sha256").read_text().strip()
    if checksum != f"{digest(archive)}  {archive.name}":
        raise ValueError("archive digest mismatch")
    allowed = {"sunshine-client.exe" if windows else "sunshine-client", "README.md", "LICENSE", "manifest.json", "SHA256SUMS", "bootstrap.example.json"}
    allowed.update(["install-windows.ps1", "uninstall-windows.ps1"] if windows else ["install-macos.sh", "uninstall-macos.sh", "org.sarmg.sunshine-client.plist"] if macos else ["install-linux.sh", "uninstall-linux.sh", "sunshine-client.service"])
    root = destination / name
    root.mkdir()
    seen = set()
    budget = 128 * 1024 * 1024

    def write(path, size, data):
        nonlocal budget
        parts = path.split("/")
        if len(parts) != 2 or parts[0] != name or parts[1] not in allowed or parts[1] in seen:
            raise ValueError("unexpected or duplicate archive entry")
        budget -= size
        if size < 0 or budget < 0:
            raise ValueError("archive budget exceeded")
        payload = data.read(size + 1)
        if len(payload) != size:
            raise ValueError("archive entry size mismatch")
        with (root / parts[1]).open("xb") as output:
            output.write(payload)
        seen.add(parts[1])

    if windows:
        with zipfile.ZipFile(archive) as source:
            for entry in source.infolist():
                if entry.is_dir() or (entry.external_attr >> 16) & 0o170000 not in (0, 0o100000):
                    raise ValueError("non-file ZIP entry")
                with source.open(entry) as data:
                    write(entry.filename, entry.file_size, data)
    else:
        with tarfile.open(archive) as source:
            for entry in source:
                if entry.isdir() and entry.name == name:
                    continue
                if not entry.isfile():
                    raise ValueError("non-file TAR entry")
                with source.extractfile(entry) as data:
                    write(entry.name, entry.size, data)
    if seen != allowed:
        raise ValueError("incomplete package")
    manifest = json.loads((root / "manifest.json").read_text())
    if manifest["source_commit"] != sha or manifest["target"] != target or manifest["protocol"] != "sunshine-management/1" or manifest["product"] != "sunshine-client":
        raise ValueError("package identity mismatch")
    if name != f"sunshine-client-{manifest['version']}-{target}" or manifest["authenticode_signed"] is not False:
        raise ValueError("package version or signature declaration mismatch")
    files = allowed - {"manifest.json", "SHA256SUMS"}
    if manifest["files"] != {p: digest(root / p) for p in sorted(files)}:
        raise ValueError("manifest digest mismatch")
    expected_checksums = "".join(f"{digest(root / p)}  {p}\n" for p in sorted(allowed - {"SHA256SUMS"}))
    if (root / "SHA256SUMS").read_text() != expected_checksums:
        raise ValueError("file checksum mismatch")
    binary = root / ("sunshine-client.exe" if windows else "sunshine-client")
    binary.chmod(0o755)
    if run(str(binary), "--version") != f"sunshine-client {manifest['version']} (git {sha}; sunshine-management/1)":
        raise ValueError("executable identity mismatch")
    return root, binary


def install_test(root, binary, temporary, installer=None, seed=None):
    # This test intentionally creates a real service/account, but never on a user's host.
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise ValueError("--install is restricted to disposable GitHub-hosted runners")
    windows = platform.system() == "Windows"
    ca = temporary / "ca.pem"
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(temporary / "key.pem"), "-out", str(ca), "-days", "1", "-subj", "/CN=Client installation test", "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "subjectAltName=IP:127.0.0.1"], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    fixture = temporary / "private-bootstrap"
    fixture.mkdir(mode=0o700)
    bootstrap = fixture / "bootstrap.json"
    bootstrap.write_text(json.dumps({"manager_endpoint": "wss://127.0.0.1:9/sunshine-client/v1/connect", "enrollment_token": "a" * 64, "sunshine_endpoint": "https://127.0.0.1:47990/", "sunshine_username": "test", "sunshine_password": "installation-fixture-only", "restart_allowed": False}), encoding="utf-8")
    bootstrap.chmod(0o600)
    if windows:
        # Protect only the secret fixture, not extracted executable/scripts. Set explicit
        # file ACEs as well as directory inheritance; recursive icacls can remove access
        # from archive files when it disables inherited ACEs at every descendant.
        escaped = str(fixture).replace("'", "''")
        powershell(f"""
$current=[Security.Principal.WindowsIdentity]::GetCurrent().User
foreach($path in @('{escaped}', '{escaped}\\bootstrap.json')) {{
  $directory=(Get-Item -LiteralPath $path).PSIsContainer
  $acl=if($directory){{New-Object System.Security.AccessControl.DirectorySecurity}}else{{New-Object System.Security.AccessControl.FileSecurity}}
  $acl.SetAccessRuleProtection($true,$false)
  $acl.SetOwner($current)
  foreach($sid in @($current.Value,'S-1-5-18','S-1-5-32-544')) {{
    $identity=New-Object System.Security.Principal.SecurityIdentifier $sid
    $inherit=if($directory){{[System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit'}}else{{[System.Security.AccessControl.InheritanceFlags]::None}}
    $rule=New-Object System.Security.AccessControl.FileSystemAccessRule($identity,'FullControl',$inherit,'None','Allow')
    $acl.AddAccessRule($rule)
  }}
  Set-Acl -LiteralPath $path -AclObject $acl
}}
""")
        install = ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(root / "install-windows.ps1"), "-Binary", str(binary), "-Bootstrap", str(bootstrap)]
        if installer:
            subprocess.run(['msiexec.exe', '/i', str(installer), '/qn', '/norestart'], check=True)
            installed = Path(os.environ['ProgramFiles']) / 'SunshineClient'
            if (installed / 'sunshine-client-tray.exe').exists():
                raise ValueError('CLI installer contains a tray executable')
            subprocess.run([str(installed / 'sunshine-client.exe'), 'init', '--state', str(Path(os.environ['ProgramData']) / 'SunshineClient'), '--bootstrap', str(bootstrap)], check=True, timeout=60)
            if powershell("(Get-Service SunshineClient).Status") != 'Stopped':
                raise ValueError('unpaired service was started by installer')

        else:
            subprocess.run(install, check=True, env=powershell_environment())
        if subprocess.run(install, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=powershell_environment()).returncode == 0:
            raise ValueError("installer overwrote an existing installation")

        if seed:
            exercise_native_service(Path(os.environ['ProgramFiles']) / 'SunshineClient/sunshine-client.exe', seed, Path(os.environ['ProgramData']) / 'SunshineClient')
        if installer:
            subprocess.run(['msiexec.exe', '/x', str(installer), '/qn', '/norestart'], check=True)
            if (installed / 'sunshine-client-tray.exe').exists():
                raise ValueError('MSI uninstall retained tray executable')
        else:
            subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(root / "uninstall-windows.ps1")], check=True, env=powershell_environment())
        if not (Path(os.environ["ProgramData"]) / "SunshineClient/provisioning/state.sqlite3").is_file():
            raise ValueError("uninstall removed protected state")
    else:
        install = ["bash", str(root / "install-linux.sh"), str(binary), str(bootstrap)]
        if installer:
            subprocess.run(['dpkg', '--install', str(installer)], check=True)
            # Offline installation is separate from successful online pairing.
            subprocess.run([str(binary), 'init', '--state', '/var/lib/sunshine-client', '--bootstrap', str(bootstrap)], check=True)
            subprocess.run(['chown', '-R', 'sunshine-client:sunshine-client', '/var/lib/sunshine-client'], check=True)
        else:
            subprocess.run(install, check=True)
        if subprocess.run(install, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
            raise ValueError("installer overwrote an existing installation")
        if subprocess.run(["systemctl", "is-active", "--quiet", "sunshine-client.service"]).returncode == 0:
            raise ValueError("unpaired service was started by installer")
        if subprocess.run(["systemctl", "is-enabled", "--quiet", "sunshine-client.service"]).returncode == 0:
            raise ValueError("installer enabled the service without administrator action")
        pending_run = subprocess.run([str(binary), "run", "--state", "/var/lib/sunshine-client", "--format", "json"], capture_output=True, timeout=60)
        if pending_run.returncode != 4:
            raise ValueError("unpaired run did not return awaiting_pairing")
        state = Path("/var/lib/sunshine-client/provisioning")
        if (state.stat().st_mode & 0o077) != 0 or not (state / "bootstrap.json").is_file():
            raise ValueError("protected pending configuration missing")
        if seed:
            exercise_native_service(Path('/opt/sunshine-client/sunshine-client'), seed, Path('/var/lib/sunshine-client'), 'sunshine-client:sunshine-client')
        if installer:
            subprocess.run(['dpkg', '--remove', 'sunshine-client'], check=True)
        else:
            subprocess.run(["bash", str(root / "uninstall-linux.sh")], check=True)
        if not (state / "bootstrap.json").is_file():
            raise ValueError("uninstall removed protected state")
    print("Native offline install, explicit-start policy, overwrite refusal and state-preserving uninstall passed; online service behavior requires enrolled-device acceptance.")


def exercise_native_service(binary, seed, state, service_user=None):
    import importlib.util
    spec = importlib.util.spec_from_file_location("native_service_acceptance", Path(__file__).with_name("native-service-acceptance.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.exercise(binary, seed, state, service_user)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--install", action="store_true")
    parser.add_argument("--installer", type=Path, help="MSI/DEB to exercise instead of script installation")
    parser.add_argument("--seed", type=Path, help="native CI fixture executable for real service/IPC acceptance")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.sha):
        parser.error("full source SHA required")
    with tempfile.TemporaryDirectory(prefix="client-check-") as tmp:
        temporary = Path(tmp)
        root, binary = verify(args.archive.resolve(), temporary, args.sha)
        print("Independent archive, manifest, checksums and executable identity verified.")
        if args.install:
            if args.installer:
                expected = args.installer.with_name(args.installer.name + '.manifest.json')
                manifest = json.loads(expected.read_text())
                if manifest['source_commit'] != args.sha or manifest['sha256'] != digest(args.installer):
                    raise ValueError('installer source or checksum mismatch')
            install_test(root, binary, temporary, args.installer.resolve() if args.installer else None, args.seed.resolve() if args.seed else None)


if __name__ == "__main__":
    main()
