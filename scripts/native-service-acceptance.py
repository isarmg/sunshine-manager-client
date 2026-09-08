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
            raise RuntimeError(f"{args[:2]} returned {result.returncode}, expected {expected}")
        value = json.loads(result.stdout)
        return value["result"] if expected == 0 else value

    initial = cli("service", "status")
    if not initial["installed"] or initial["state"] != "stopped":
        raise RuntimeError("installation must leave the Client stopped")
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
