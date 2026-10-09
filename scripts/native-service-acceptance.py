#!/usr/bin/env python3
"""Actual SCM/systemd/launchd tests with synthetic offline identity in disposable CI.

This is service and storage acceptance, not online pairing or real Sunshine acceptance.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import time


def execute(command, **kwargs):
    return subprocess.run([str(v) for v in command], capture_output=True, text=True, timeout=75, **kwargs)


def exercise(binary, seed, state, service_user=None):
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise RuntimeError("native installation acceptance requires a disposable hosted runner")
    binary, seed, state = map(Path, (binary, seed, state))
    if not all(p.is_absolute() for p in (binary, seed, state)):
        raise RuntimeError("absolute paths required")

    def cli(*args, expected=0, input=None):
        result = execute([binary, *args, "--state", state, "--format", "json", "--timeout", "30s", "--non-interactive"], input=input)
        if result.returncode != expected:
            raise RuntimeError(f"{args[:2]} returned {result.returncode}, expected {expected}; CLI response: {result.stdout[:4096]}")
        value = json.loads(result.stdout)
        return value["result"] if expected == 0 else value

    initial = cli("service", "status")
    if not initial["installed"] or initial["state"] != "stopped":
        raise RuntimeError("installation must leave the Client stopped")
    if platform.system() == "Windows" and initial["startup"] != "automatic":
        raise RuntimeError("new Windows Client service must use native automatic system startup")
    if platform.system() == "Linux" and initial["startup"] != "enabled":
        raise RuntimeError("fresh Linux installation must enable systemd startup before pairing")
    if execute([seed, state]).returncode:
        raise RuntimeError("protected acceptance fixture could not be created")
    if service_user and execute(["chown", "-R", service_user, state]).returncode:
        raise RuntimeError("could not assign newly seeded CI state to service account")
    records = cli("tasks", "list")
    binding = cli("pair", "status")["pairing"]["binding"]

    def live_status(previous_epoch=None):
        deadline = time.monotonic() + 20
        while True:
            live = cli("status")
            runtime = live["runtime"]
            if runtime.get("available") and runtime["service_epoch"] != previous_epoch:
                return live
            if time.monotonic() >= deadline:
                raise RuntimeError("running service did not expose a fresh authenticated status")
            time.sleep(0.2)

    try:
        cli("service", "enable", "--now")
        running = cli("service", "status")
        assert running["state"] == "running", running
        live = live_status()
        first_epoch = live["runtime"]["service_epoch"]
        if platform.system() == "Windows":
            logs = cli("logs", "--tail", "100", "--event", "xscc.windows.started")
            assert logs["source"] == "private-runtime-log" and logs["retention_bytes"] == 40*1024*1024
            assert logs["entries"] and all(row["event"] == "xscc.windows.started" and row["timestamp"].endswith("Z") for row in logs["entries"])
            log_path = str(state / "logs").replace("'", "''")
            acl_script = "$ErrorActionPreference='Stop'; $a=Get-Acl -LiteralPath '" + log_path + "'; $owner=$a.GetOwner([System.Security.Principal.SecurityIdentifier]).Value; Write-Output ('owner='+$owner+' protected='+$a.AreAccessRulesProtected); if(-not $a.AreAccessRulesProtected -or $owner -ne 'S-1-5-18'){exit 1}; foreach($r in $a.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])){Write-Output ('trustee='+$r.IdentityReference.Value+' access='+$r.AccessControlType); if($r.AccessControlType -ne 'Allow' -or $r.IdentityReference.Value -notin @('S-1-5-18','S-1-5-32-544')){exit 2}}; exit 0"
            # Match the package checker's Windows PowerShell 5 environment:
            # a pwsh-hosted parent must not inject PowerShell 7 modules.
            powershell_env = {key: value for key, value in os.environ.items() if key.lower() != "psmodulepath"}
            acl = execute(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", acl_script], env=powershell_env)
            assert acl.returncode == 0, f"runtime log folder ACL failed: exit={acl.returncode}; {acl.stdout[:1024]}; {acl.stderr[:1024]}"
            raw = execute([binary, "logs", "--state", state, "--format", "ndjson", "--follow", "--timeout", "2s"])
            assert raw.returncode == 0 and raw.stdout.strip(), "typed Windows log follow did not expose service diagnostics"
            assert all(json.loads(line)["ok"] for line in raw.stdout.splitlines())
        assert live["health"] == "unknown", "offline fixture must not report healthy"
        cli("status", "--check", expected=12)
        cli("credentials", "update", "--input-stdin", input=json.dumps({"sunshine_username": "blocked", "sunshine_password": "not-committed"}), expected=5)
        assert cli("tasks", "list") == records
        cli("service", "stop")
        stopped = cli("service", "status")
        assert stopped["state"] == "stopped" and stopped["startup"] == running["startup"]
        time.sleep(2)
        assert cli("service", "status")["state"] == "stopped", "manual stop was immediately restarted"
        cli("credentials", "update", "--input-stdin", input=json.dumps({"sunshine_username": "updated", "sunshine_password": "new-offline-fixture-secret"}))
        assert cli("tasks", "list") == records
        cli("service", "start")
        live_status(first_epoch)
        cli("service", "disable", "--now")
        assert cli("service", "status")["state"] == "stopped"
        startup = cli("service", "status")["startup"]
        cli("service", "start")
        assert cli("service", "status")["startup"] == startup
        cli("service", "stop")
        assert cli("tasks", "list") == records
        assert cli("pair", "status")["pairing"]["binding"] == binding
    finally:
        cli("service", "disable", "--now")
    print(json.dumps({"native_os": platform.system(), "service_lifecycle": "passed", "authenticated_status": "passed", "journal_preserved": True, "online_pairing": "not_exercised"}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--seed", required=True, type=Path)
    parser.add_argument("--state", required=True, type=Path)
    parser.add_argument("--service-user")
    args = parser.parse_args()
    exercise(args.binary, args.seed, args.state, args.service_user)


if __name__ == "__main__":
    main()
