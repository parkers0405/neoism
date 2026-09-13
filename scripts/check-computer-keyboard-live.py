#!/usr/bin/env python3
"""Bounded, device-isolated Hyprland/Fcitx keyboard regression experiment.

By default no host display, input devices, session bus, or config is passed through.
Logs are retained in the printed temporary directory. Hyprland 0.56.2 with
Aquamarine 0.14.0 rejects pure headless startup: no allocator is available.
--render-node exposes exactly one validated render node for an allocator probe;
it is NOT a known-working headless route on that version. Primary DRM nodes,
input devices and the user's compositor are never exposed by default.
--nested explicitly permits a normal host Wayland window for the compositor
only. Its exec clients run in a second mount namespace hiding that host socket,
and verify their private display and Hyprland IPC endpoints before starting.
"""
import argparse
import hashlib
import os
import pathlib
import re
import shlex
import stat
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--render-node", type=pathlib.Path,
                        help="Expose only this /dev/dri/renderD* character device (allocator probe)")
    parser.add_argument("--nested", action="store_true",
                        help="Allow compositor-only host Wayland presentation; clients remain private")
    parser.add_argument("--trace-wayland", action="store_true",
                        help="Also log test client protocol traffic; Fcitx trace is always required for proof")
    args = parser.parse_args()
    host_socket = None
    if args.nested:
        display = pathlib.Path(os.environ["WAYLAND_DISPLAY"])
        host_socket = (pathlib.Path(os.environ["XDG_RUNTIME_DIR"]) / display).resolve(strict=True)
        if not stat.S_ISSOCK(host_socket.stat().st_mode):
            parser.error("Host Wayland endpoint is not a socket")
        if args.render_node is None:
            parser.error("--nested requires an explicit --render-node")
    render = args.render_node
    if render is not None:
        render = render.resolve(strict=True)
        info = render.stat()
        if not (render.parent == pathlib.Path("/dev/dri")
                and render.name.startswith("renderD")
                and render.name[7:].isdigit()
                and stat.S_ISCHR(info.st_mode)
                and os.major(info.st_rdev) == 226
                and 128 <= os.minor(info.st_rdev) <= 255):
            parser.error("Only a real DRM render node is allowed; never primary/input devices")
    if os.environ.get("NEOISM_LIVE_KEYBOARD_TEST") != "1":
        sys.exit("Explicitly set NEOISM_LIVE_KEYBOARD_TEST=1")
    root = pathlib.Path(__file__).resolve().parents[1]
    run = pathlib.Path(tempfile.mkdtemp(prefix="nk-live-"))
    for name in ("r", "home", "config", "cache", "data", "state"):
        (run / name).mkdir(mode=0o700)
    # Only dependency caches and the tiny harness build cache remain writable.
    # Neither desktop config nor desktop runtime sockets are visible to clients.
    cargo = pathlib.Path(os.environ.get("CARGO_HOME", pathlib.Path.home() / ".cargo"))
    rustup = pathlib.Path(os.environ.get("RUSTUP_HOME", pathlib.Path.home() / ".rustup"))
    target = root / "target/computer-keymap-regression"
    target.mkdir(parents=True, exist_ok=True)
    source_dir = root / "neoism-agent/crates/neoism-agent-server/src/computer_use"
    (run / "sources.sha256").write_text("".join(
        f"{hashlib.sha256((source_dir / name).read_bytes()).hexdigest()}  {name}\n"
        for name in ("linux_text.rs", "shortcuts.rs", "linux_keyboard_live_tests.rs")))
    quote = shlex.quote
    trace = "env WAYLAND_DEBUG=client " if args.trace_wayland else ""
    child = run / "client.sh"
    child.write_text(f'''#!/bin/sh
set -eu
exec > {quote(str(run / 'session.log'))} 2>&1
unset WAYLAND_SOCKET DISPLAY
python3 - <<'VERIFY'
import os, pathlib, stat
runtime = pathlib.Path(os.environ["XDG_RUNTIME_DIR"])
assert runtime == pathlib.Path({str(run / 'r')!r})
display = pathlib.Path(os.environ["WAYLAND_DISPLAY"])
assert not display.is_absolute() and len(display.parts) == 1, display
endpoint = runtime / display
assert stat.S_ISSOCK(endpoint.stat().st_mode), endpoint
signature = os.environ["HYPRLAND_INSTANCE_SIGNATURE"]
assert '/' not in signature and signature
ipc = runtime / 'hypr' / signature / '.socket.sock'
assert stat.S_ISSOCK(ipc.stat().st_mode), ipc
assert not pathlib.Path('/mnt/wayland').exists(), 'Host socket visible to test!'
assert not pathlib.Path('/dev/input').exists()
assert not list(pathlib.Path('/dev/dri').glob('card*'))
print('Verified private display:', endpoint, 'inode:', endpoint.stat().st_ino, flush=True)
print('Verified private IPC:', ipc, flush=True)
print('Private DBus:', os.environ['DBUS_SESSION_BUS_ADDRESS'], flush=True)
VERIFY
printf 'WAYLAND_DISPLAY=%s\\nHYPRLAND_INSTANCE_SIGNATURE=%s\\n' "$WAYLAND_DISPLAY" "$HYPRLAND_INSTANCE_SIGNATURE"
env WAYLAND_DEBUG=client fcitx5 > {quote(str(run / 'fcitx.log'))} 2>&1 &
ime=$!
trap 'kill "$ime" 2>/dev/null || :; wait "$ime" 2>/dev/null || :' EXIT
# This bus exists only inside dbus-run-session, never the user's session bus.
i=0
until dbus-send --session --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.NameHasOwner string:org.fcitx.Fcitx5 | grep -q 'boolean true'; do
    kill -0 "$ime"
    i=$((i + 1)); [ "$i" -lt 100 ] || exit 1
    sleep .1
done
sleep 1
set +e
{trace}python3 {quote(str(root / 'scripts/check-computer-keymap.py'))} hyprland_production_keyboard_roundtrip -- --ignored --nocapture > {quote(str(run / 'test.log'))} 2>&1
status=$?
printf '%s\\n' "$status" > {quote(str(run / 'test.status'))}
# Exact private instance inherited from this compositor; never discovers host instances.
hyprctl -i "$HYPRLAND_INSTANCE_SIGNATURE" dispatch exit
exit "$status"
''')
    config = run / "hyprland.conf"
    # The outer socket is visible only to Hyprland. Even a mistaken client
    # display variable cannot reach it after this second namespace boundary.
    client_command = f"bwrap --bind / / --dev /dev --tmpfs /mnt -- /bin/sh {quote(str(child))}"
    config.write_text(f'''monitor = {"WAYLAND-1" if args.nested else "HEADLESS-1"}, 1280x720@60, 0x0, 1
exec-once = {client_command}
xwayland {{
    enabled = false
}}
animations {{
    enabled = false
}}
misc {{
    disable_hyprland_logo = true
    disable_splash_rendering = true
    force_default_wallpaper = 0
}}
debug {{
    disable_logs = false
    enable_stdout_logs = true
}}
''')
    env = {
        "PATH": f"{cargo}/bin:/usr/bin:/bin",
        "HOME": str(run / "home"),
        "CARGO_HOME": str(cargo), "RUSTUP_HOME": str(rustup),
        "XDG_RUNTIME_DIR": str(run / "r"),
        "XDG_CONFIG_HOME": str(run / "config"),
        "XDG_CACHE_HOME": str(run / "cache"),
        "XDG_DATA_HOME": str(run / "data"),
        "XDG_STATE_HOME": str(run / "state"),
        "XDG_CURRENT_DESKTOP": "Hyprland", "XDG_SESSION_TYPE": "wayland",
        "HYPRLAND_NO_SD_VARS": "1", "HYPRLAND_NO_SD_NOTIFY": "1",
        "HYPRLAND_NO_CRASHREPORTER": "1",
        "LIBGL_ALWAYS_SOFTWARE": "1", "GALLIUM_DRIVER": "llvmpipe",
        "LIBSEAT_BACKEND": "noop",  # No logind/seatd or VT acquisition.
        "NEOISM_LIVE_KEYBOARD_TEST": "1", "LANG": "C.UTF-8",
    }
    if host_socket is not None:
        env["WAYLAND_DISPLAY"] = "/mnt/wayland"
    if render is not None:
        env["AQ_DRM_DEVICES"] = str(render)
        env.pop("LIBGL_ALWAYS_SOFTWARE")
        env.pop("GALLIUM_DRIVER")
    command = [
        "bwrap", "--die-with-parent", "--unshare-all", "--new-session",
        "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc",
        "--tmpfs", "/run", "--tmpfs", "/tmp",
        "--tmpfs", "/mnt",
        *(["--ro-bind", str(host_socket), "/mnt/wayland"] if host_socket is not None else []),
        *(["--dev-bind", str(render), str(render)] if render is not None else []),
        "--bind", str(run), str(run),
        "--bind", str(cargo), str(cargo), "--bind", str(target), str(target),
        "--chdir", str(root), "--",
        "dbus-run-session", "--", "Hyprland", "--config", str(config),
    ]
    (run / "launch.txt").write_text(shlex.join(command) + "\n" + repr(env) + "\n")
    print(f"Isolated logs: {run}", flush=True)
    with (run / "hyprland.log").open("w") as log:
        process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            status = process.wait(timeout=110)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            # Kill only our bwrap supervisor. Its PID namespace tears down all
            # private children, including processes which daemonized/setsid.
            process.kill()
            process.wait()
            status = 124
    print(f"Isolated compositor exit: {status}")
    for name in ("session.log", "test.status", "test.log", "fcitx.log", "hyprland.log"):
        path = run / name
        if path.exists():
            print(f"--- {name} (last 50 lines) ---")
            print("\n".join(path.read_text(errors="replace").splitlines()[-50:]))
    # A receiver pass alone is insufficient: without this evidence the path
    # could silently bypass the IME. Only the private Fcitx gets traced here.
    fcitx_log = run / "fcitx.log"
    grabbed = [line for line in fcitx_log.read_text(errors="replace").splitlines()
               if re.search(r"zwp_input_method_keyboard_grab_v2[#@]\d+\.(key|modifiers|keymap)\(", line)] if fcitx_log.exists() else []
    counts = {event: sum(f".{event}(" in line for line in grabbed)
              for event in ("key", "modifiers", "keymap")}
    states = re.findall(r"\.key\(\d+,\s*\d+,\s*\d+,\s*([01])\)", "\n".join(grabbed))
    evidence = f"Fcitx grabbed events: {counts}; presses={states.count('1')}, releases={states.count('0')}"
    (run / "ime-evidence.txt").write_text(evidence + "\n" + "\n".join(grabbed) + "\n")
    print(evidence)
    if not (run / "test.status").exists():
        print("BLOCKED: compositor/client startup did not reach test completion; no live pass claimed.")
        return status or 1
    test_status = int((run / "test.status").read_text())
    if test_status == 0 and not (all(counts.values()) and "1" in states and "0" in states):
        print("FAIL: receiver passed but Fcitx grab lacks key press/release, modifiers or keymap evidence")
        return 1
    return test_status


if __name__ == "__main__":
    sys.exit(main())
