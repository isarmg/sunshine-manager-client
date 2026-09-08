#!/usr/bin/env python3
"""Build a source-bound Client archive on its native OS; never package the Server."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
COMMON_FILES = {"LICENSE-APACHE": "LICENSE", "README.md": "README.md", "deploy/bootstrap.example.json": "bootstrap.example.json"}


def run(*args, **kwargs):
    return subprocess.check_output(args, cwd=ROOT, text=True, **kwargs).strip()


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def package_files(directory):
    # WindowsPath sorts case-insensitively; the wire format uses exact ASCII names.
    return sorted(directory.iterdir(), key=lambda path: path.name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--require-tag", action="store_true")
    args = parser.parse_args()
    for source in COMMON_FILES:
        if not (ROOT / source).is_file():
            parser.error(f"required package input missing: {source}")
    if not args.output.is_absolute() or args.output.exists():
        parser.error("output must be a new absolute directory (no overwrites)")
    if run("git", "status", "--porcelain", "--untracked-files=all"):
        parser.error("packaging requires a clean committed source tree")
    sha = run("git", "rev-parse", "HEAD")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-rc\.[0-9]+)?", version):
        parser.error("unsafe Client version")
    if args.require_tag:
        tag = f"v{version}"
        if run("git", "cat-file", "-t", tag) != "tag" or run("git", "rev-parse", tag + "^{commit}") != sha:
            parser.error("annotated Client tag must resolve to this exact source")
    machine = platform.machine().lower()
    windows = platform.system() == "Windows"
    macos = platform.system() == "Darwin"
    if macos and machine in ("arm64", "aarch64", "x86_64"):
        target = ("x86_64" if machine == "x86_64" else "aarch64") + "-apple-darwin"
    elif machine in ("x86_64", "amd64") and (windows or (platform.system() == "Linux" and platform.libc_ver()[0] == "glibc")):
        target = "x86_64-pc-windows-msvc" if windows else "x86_64-unknown-linux-gnu"
    else:
        parser.error("unsupported native OS/architecture")
    executable = "sunshine-client.exe" if windows else "sunshine-client"
    env = dict(os.environ, SUNSHINE_CLIENT_BUILD_SHA=sha, CARGO_INCREMENTAL="0")
    if windows:
        env["RUSTFLAGS"] = env.get("RUSTFLAGS", "") + " -C target-feature=+crt-static"
    subprocess.run(["cargo", "build", "--locked", "--release", "-p", "sunshine-client", "--target", target], cwd=ROOT, env=env, check=True)
    metadata = json.loads(run("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"))
    binary = Path(metadata["target_directory"]) / target / "release" / executable
    expected = f"sunshine-client {version} (git {sha}; sunshine-management/1)"
    if run(str(binary), "--version") != expected:
        raise RuntimeError("binary source identity mismatch")
    name = f"sunshine-client-{version}-{target}"
    with tempfile.TemporaryDirectory(prefix="client-package-") as tmp:
        stage = Path(tmp) / name
        stage.mkdir()
        shutil.copy2(binary, stage / executable)
        for source, destination in COMMON_FILES.items():
            shutil.copy2(ROOT / source, stage / destination)
        for item in (["install-windows.ps1", "uninstall-windows.ps1"] if windows else ["macos/install-macos.sh", "macos/uninstall-macos.sh", "macos/org.sarmg.sunshine-client.plist"] if macos else ["install-linux.sh", "uninstall-linux.sh", "sunshine-client.service"]):
            shutil.copy2(ROOT / "deploy" / item, stage / Path(item).name)
        manifest = {"product": "sunshine-client", "version": version, "source_commit": sha, "target": target,
                    "protocol": "sunshine-management/1", "authenticode_signed": False, "native_acceptance": "required", "notarized": False,
                    "files": {p.name: digest(p) for p in package_files(stage)}}
        (stage / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        (stage / "SHA256SUMS").write_text("".join(f"{digest(p)}  {p.name}\n" for p in package_files(stage)), encoding="utf-8")
        args.output.mkdir(parents=True, exist_ok=False)
        archive = args.output / (name + (".zip" if windows else ".tar.gz"))
        if windows:
            with zipfile.ZipFile(archive, "x", zipfile.ZIP_DEFLATED) as out:
                for p in package_files(stage):
                    out.write(p, f"{name}/{p.name}")
        else:
            with tarfile.open(archive, "x:gz") as out:
                out.add(stage, arcname=name)
        (args.output / (archive.name + ".sha256")).write_text(f"{digest(archive)}  {archive.name}\n", encoding="utf-8")
    if windows:
        # MSI's ProductVersion is numeric; the source version remains in its manifest.
        subprocess.run(["powershell.exe", "-NoProfile", "-File", str(ROOT / "scripts/build-windows-installer.ps1"),
                        "-ClientExe", str(binary), "-Output", str(args.output), "-Version", version.split("-")[0]],
                       check=True, env={key: value for key, value in os.environ.items() if key.lower() != "psmodulepath"})
        numeric_version = version.split("-")[0]
        (args.output / f"sunshine-client-{numeric_version}-windows-x64.msi").rename(
            args.output / f"sunshine-client-{version}-windows-x64.msi")
    elif not macos:
        subprocess.run(["python3", str(ROOT / "scripts/build-linux-installer.py"), "--binary", str(binary),
                        "--output", str(args.output), "--version", version], check=True)
    for installer in sorted(args.output.glob("*.msi")) + sorted(args.output.glob("*.deb")):
        installer.with_name(installer.name + ".sha256").write_text(f"{digest(installer)}  {installer.name}\n")
        installer.with_name(installer.name + ".manifest.json").write_text(json.dumps({
            "product": "sunshine-client", "version": version, "source_commit": sha,
            "authenticode_signed": False, "native_acceptance": "required", "notarized": False, "sha256": digest(installer), "target": target,
        }, indent=2) + "\n")
    if run("git", "rev-parse", "HEAD") != sha or run("git", "status", "--porcelain", "--untracked-files=all"):
        raise RuntimeError("source changed while packaging; do not publish output")
    print(f"Built {archive.name} at {sha}; installation and real Sunshine acceptance are separate gates.")


if __name__ == "__main__":
    main()
