#!/usr/bin/env python3
"""One-shot hook: snap the NEXT window you open to center, then exit.

The counterpart to auto_place.py — a one-shot hook meant for a keybinding rather
than [autostart]. It connects, waits for a single window to open, positions it,
and exits (that final exit is what makes it "one-shot"; a daemon would loop).

Bind it to a key in spatil.toml:

    [keybindings]
    "Super+g" = "hook place_next.py"

Then press the key and open a window — it lands centered instead of on the grid.
Needs only python3; SPATIL_SOCKET is set for you when launched via a keybinding.
"""

import json
import os
import socket
import sys

CENTER = [0.0, 0.0, -3.0]

SOCKET_NAME = os.environ.get("SPATIL_SOCKET", "spatil-0")


def socket_path(name):
    base = os.environ.get("XDG_RUNTIME_DIR") or "/tmp"
    return os.path.join(base, f"{name}.sock")


def main():
    path = socket_path(SOCKET_NAME)
    if not os.path.exists(path):
        sys.exit(f"{path} not found — is Spatil running?")

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(path)

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
            command = {
                "command": "move_cluster",
                "window_id": event["window_id"],
                "args": CENTER,
            }
            sock.sendall((json.dumps(command) + "\n").encode())
            return  # placed one window — done


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
