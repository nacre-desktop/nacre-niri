#!/usr/bin/env python3
"""Exercise fork and stock CLI IPC against a private protocol fixture."""

import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
from threading import Thread

def request(binary, args, expected, reply):
    with tempfile.TemporaryDirectory(prefix="nacre-niri-ipc-") as temp:
        socket_path = Path(temp) / "niri.sock"
        errors = []
        server = socket.socket(socket.AF_UNIX)
        server.settimeout(5)
        server.bind(str(socket_path))
        server.listen(1)

        def serve():
            try:
                connection, _ = server.accept()
                connection.settimeout(5)
                with connection:
                    data = bytearray()
                    while not data.endswith(b"\n"):
                        chunk = connection.recv(4096)
                        if not chunk:
                            raise AssertionError("IPC client closed before its request line")
                        data.extend(chunk)
                    value = json.loads(data)
                    if value != expected:
                        raise AssertionError(f"unexpected IPC request: {value!r}")
                    connection.sendall(json.dumps({"Ok": reply}).encode() + b"\n")
            except Exception as error:  # re-raise a fixture thread failure in the caller
                errors.append(error)
            finally:
                server.close()

        server_thread = Thread(target=serve, daemon=True)
        server_thread.start()
        result = subprocess.run(
            [str(binary), "msg", "--json", *args],
            check=False,
            capture_output=True,
            text=True,
            env={**os.environ, "NIRI_SOCKET": str(socket_path)},
            timeout=10,
        )
        server_thread.join(timeout=10)
        server.close()
        if server_thread.is_alive():
            raise AssertionError("IPC fixture did not finish")
        if errors:
            raise errors[0]
        if result.returncode != 0:
            raise AssertionError(f"{binary} msg {' '.join(args)} failed: {result.stderr}")
        return json.loads(result.stdout)


def event_stream(binary, request, expected, event):
    with tempfile.TemporaryDirectory(prefix="nacre-niri-stream-") as temp:
        socket_path = Path(temp) / "niri.sock"
        errors = []
        server = socket.socket(socket.AF_UNIX)
        server.settimeout(5)
        server.bind(str(socket_path))
        server.listen(1)

        def serve():
            try:
                connection, _ = server.accept()
                connection.settimeout(5)
                with connection:
                    data = bytearray()
                    while not data.endswith(b"\n"):
                        chunk = connection.recv(4096)
                        if not chunk:
                            raise AssertionError("event-stream client closed before its request")
                        data.extend(chunk)
                    value = json.loads(data)
                    if value != expected:
                        raise AssertionError(f"unexpected event-stream request: {value!r}")
                    connection.sendall(
                        json.dumps({"Ok": "Handled"}).encode()
                        + b"\n"
                        + json.dumps(event).encode()
                        + b"\n"
                    )
            except Exception as error:
                errors.append(error)
            finally:
                server.close()

        server_thread = Thread(target=serve, daemon=True)
        server_thread.start()
        result = subprocess.run(
            [str(binary), "msg", "-j", "event-stream", *request],
            check=False,
            capture_output=True,
            text=True,
            env={**os.environ, "NIRI_SOCKET": str(socket_path)},
            timeout=10,
        )
        server_thread.join(timeout=10)
        server.close()
        if server_thread.is_alive():
            raise AssertionError("event-stream fixture did not finish")
        if errors:
            raise errors[0]
        if "CollectionsChanged" not in result.stdout and "WorkspacesChanged" not in result.stdout:
            raise AssertionError(f"event stream did not print its fixture event: {result.stdout!r}")
        return result.stdout


def main():
    fork, stock = map(Path, sys.argv[1:3])
    version = subprocess.run(
        [str(fork), "--version"], check=True, capture_output=True, text=True
    ).stdout.strip()
    match = re.search(r"nacre-niri ([0-9a-f]{40})", version)
    if not match:
        raise AssertionError(f"fork identity with full revision missing from version: {version}")
    expected_version = f"26.04 (nacre-niri {match.group(1)})"

    fork_reply = request(fork, ["version"], "Version", {"Version": expected_version})
    if fork_reply["compositor"] != expected_version or fork_reply["cli"] != expected_version:
        raise AssertionError(f"fork IPC version mismatch: {fork_reply!r}")

    stock_reply = request(stock, ["workspaces"], "Workspaces", {"Workspaces": []})
    if stock_reply != []:
        raise AssertionError(f"stock CLI IPC returned unexpected workspaces: {stock_reply!r}")

    legacy = event_stream(
        stock,
        [],
        "EventStream",
        {"WorkspacesChanged": {"workspaces": []}},
    )
    default = event_stream(
        fork,
        [],
        "EventStream",
        {"WorkspacesChanged": {"workspaces": []}},
    )
    extension = event_stream(
        fork,
        ["--collections"],
        "EventStreamWithCollections",
        {"CollectionsChanged": {"collections": []}},
    )
    if "WorkspacesChanged" not in legacy or "WorkspacesChanged" not in default:
        raise AssertionError("legacy event-stream request did not receive standard state")
    if "CollectionsChanged" not in extension:
        raise AssertionError("collection opt-in did not receive its event")
    print("PASS: fork identity, query IPC, stock-compatible default event streams, collections opt-in")


if __name__ == "__main__":
    main()
