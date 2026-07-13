#!/usr/bin/env python3
"""Auto-place hook: lay new windows out on a 3D grid so they never overlap.

A ready-to-run example of a Spatil automation script. Every window that opens
gets positioned by this script before it is ever drawn, so it appears already in
place (no flicker from the origin).

To have Spatil launch it for you, add it to the [autostart] table in
spatil.toml (it lives in ~/.config/spatil/hooks/ after first-run bootstrap):

    [autostart]
    exec = ["hooks/auto_place.py"]

Or run it by hand against a running Spatil:

    python3 auto_place.py            # uses socket "spatil-0"
    SPATIL_SOCKET=spatil-0 python3 auto_place.py

It only needs python3 (standard library). SPATIL_SOCKET is set for you when
launched via [autostart].
"""

import json
import os
import socket
import sys

COLUMNS = 3          # windows per row before wrapping to the next row
SPACING = 2.5        # world-units between window centers

SOCKET_NAME = os.environ.get("SPATIL_SOCKET", "spatil-0")


def socket_path(name):
    base = os.environ.get("XDG_RUNTIME_DIR") or "/tmp"
    return os.path.join(base, f"{name}.sock")


def grid_position(index):
    row, col = divmod(index, COLUMNS)
    x = (col - (COLUMNS - 1) / 2) * SPACING
    y = -row * SPACING
    return [x, y, 0.0]


def main():
    path = socket_path(SOCKET_NAME)
    if not os.path.exists(path):
        sys.exit(f"{path} not found — is Spatil running?")

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)
    print(f"auto_place connected to {path}")

    placed = 0
    buf = b""
    while True:
        chunk = sock.recv(4096)
        if not chunk:
            break
        buf += chunk
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            if not line.strip():
                continue
            event = json.loads(line)
            if event.get("event") != "window_committed":
                continue
            args = grid_position(placed)
            placed += 1
            command = {
                "command": "move_cluster",
                "window_id": event["window_id"],
                "args": args,
            }
            sock.sendall((json.dumps(command) + "\n").encode())
            print(f"placed window {event['window_id']} at {args}")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
