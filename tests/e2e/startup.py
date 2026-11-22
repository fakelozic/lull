"""How fast Lull opens, on private servers.

Times from the process starting (or the click) to the first frame: Lull's own window.first_frame log line
on KWin, and, as an independent check, the pixels themselves on Xvfb. Run it with scripts/startup.sh.
"""
import ctypes
import datetime
import glob
import json
import os
import pathlib
import re
import signal
import statistics
import subprocess
import sys
import time

if not os.path.isdir("/sandbox/run") or os.path.exists("/dev/snd") or os.path.exists("/dev/dri"):
    sys.exit("refusing to run outside the sandboxes")

sys.path.insert(0, "/usr/lib64/python3/site-packages")
import gi  # noqa: E402

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

LIMIT_WARM, LIMIT_COLD = 100, 150
LULL = "/lull/target/release/lull"
STATE = pathlib.Path("/sandbox/state/lull")
RAIN = next(pathlib.Path("/sandbox/home/Music").glob("*.opus"))
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
results = []


def read_cold_files():
    files = set(re.findall(r"(/\S+) \(0x", subprocess.run(["ldd", LULL], capture_output=True, text=True).stdout))
    for pattern in ("libpulse*", "pulseaudio/*", "libopus*", "libogg*", "libxkbcommon*", "libwayland*", "libfontconfig*"):
        files.update(glob.glob(f"/usr/lib64/{pattern}"))
    for folder in ("/usr/share/fonts", "/usr/lib/fontconfig/cache", "/usr/share/X11/xkb", "/usr/share/fontconfig/conf.avail",
                   "/sandbox/home/.local/share/fonts"):
        files.update(glob.glob(f"{folder}/**/*", recursive=True))
    files.update((LULL, str(RAIN)))
    return [file for file in files if os.path.isfile(file)]


COLD_FILES = read_cold_files()


def drop_caches():
    for file in COLD_FILES:
        try:
            fd = os.open(file, os.O_RDONLY)
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
            os.close(fd)
        except OSError:
            pass


def frames():
    found = []
    for path in sorted((STATE / "logs").glob("*.log")):
        for line in path.read_text().splitlines():
            row = json.loads(line)
            if row["event"] == "window.first_frame":
                found.append(datetime.datetime.fromisoformat(row["time"].replace("Z", "+00:00")).timestamp())
    return found


def timed(action):
    before = len(frames())
    started = time.time()
    made = action()
    while time.time() - started < 8:
        shown = frames()
        if len(shown) > before:
            return (shown[before] - started) * 1000, made
        time.sleep(0.02)
    raise RuntimeError("no frame")


def seed():
    STATE.mkdir(parents=True, exist_ok=True)
    (STATE / "state").write_bytes(f"lull-state 1\nposition={2840 * 48000}\nvolume=45\ntrack={RAIN}".encode())


def start(env=None):
    return subprocess.Popen([LULL], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def stop(proc):
    proc.send_signal(signal.SIGTERM)
    proc.wait(8)
    time.sleep(0.3)


def report(name, values, limit):
    values = sorted(values)
    typical, worst = statistics.median(values), values[-1]
    results.append(worst < limit)
    print(f"{'PASS' if worst < limit else 'FAIL'}  {name:<44} median {typical:5.1f} ms  worst {worst:5.1f} ms  (limit {limit})", flush=True)


def launches(cold, runs):
    values = []
    for index in range(runs + 1):
        if cold:
            drop_caches()
        took, proc = timed(start)
        if index:
            values.append(took)
        time.sleep(0.4)
        stop(proc)
    return values


seed()
report("launch, warm", launches(False, 20), LIMIT_WARM)
report("launch, file caches dropped", launches(True, 8), LIMIT_COLD)

running = start()
time.sleep(2)
values = []
for index in range(16):
    took, second = timed(start)
    second.wait(5)
    if index:
        values.append(took)
    time.sleep(0.5)
report("`lull` again while it runs", values, LIMIT_WARM)

item = f"org.kde.StatusNotifierItem-{running.pid}-1"


def tray_click():
    bus.call_sync(item, "/StatusNotifierItem", "org.kde.StatusNotifierItem", "ProvideXdgActivationToken",
                  GLib.Variant("(s)", ("test-token",)), None, 0, 3000, None)
    bus.call_sync(item, "/StatusNotifierItem", "org.kde.StatusNotifierItem", "Activate", GLib.Variant("(ii)", (0, 0)),
                  None, 0, 3000, None)


values = []
for index in range(16):
    tray_click()  # It has focus as the newest window, so this hides it.
    time.sleep(0.7)
    if subprocess.run(["pgrep", "-f", "^lull --window"], capture_output=True).stdout.strip():
        results.append(False)
        print("FAIL  a tray click didn't hide the focused window on Wayland", flush=True)
        break
    took, _ = timed(tray_click)
    if index:
        values.append(took)
    time.sleep(0.7)
if values:
    report("tray click, Wayland, with activation token", values, LIMIT_WARM)
tokens = sum(1 for path in (STATE / "logs").glob("*.log") for line in path.read_text().splitlines()
             if '"tray.clicked"' in line and "token=true" in line)
results.append(tokens >= 16)
print(f"{'PASS' if tokens >= 16 else 'FAIL'}  activation tokens reach the app                {tokens} clicks")
stop(running)

# The same launch judged by pixels alone, and whether that first picture is already complete.
X_ENV = {key: value for key, value in os.environ.items() if key != "WAYLAND_DISPLAY"}
X_ENV.update(DISPLAY=":99", __EGL_VENDOR_LIBRARY_FILENAMES="/usr/share/glvnd/egl_vendor.d/50_mesa.json")
subprocess.Popen(["Xvfb", ":99", "-screen", "0", "400x560x24", "-nolisten", "tcp", "-noreset", "-extension", "GLX"],
                 env=X_ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1.2)
xlib = ctypes.CDLL("libX11.so.6")
xlib.XOpenDisplay.restype = ctypes.c_void_p
xlib.XDefaultRootWindow.argtypes, xlib.XDefaultRootWindow.restype = [ctypes.c_void_p], ctypes.c_ulong
xlib.XGetImage.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint,
                           ctypes.c_ulong, ctypes.c_int]
xlib.XGetImage.restype = ctypes.c_void_p
xlib.XGetPixel.argtypes, xlib.XGetPixel.restype = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int], ctypes.c_ulong
xlib.XDestroyImage.argtypes = [ctypes.c_void_p]
display = xlib.XOpenDisplay(b":99")
root = xlib.XDefaultRootWindow(display)


def area(x, y, width, height):
    image = xlib.XGetImage(display, root, x, y, width, height, 0xFFFFFFFF, 2)
    if not image:
        return []
    pixels = [xlib.XGetPixel(image, column, row) & 0xFFFFFF for row in range(height) for column in range(width)]
    xlib.XDestroyImage(image)
    return pixels


values, incomplete = [], 0
for index in range(16):
    # The window's leaving takes a moment.
    cleared = time.monotonic() + 5
    while time.monotonic() < cleared and any(area(4, 530, 1, 1)):
        time.sleep(0.05)
    began = time.monotonic()
    proc = start(X_ENV)
    while time.monotonic() - began < 5:
        if area(4, 530, 1, 1) == [0x090D0A]:
            break
        time.sleep(0.0005)
    took = (time.monotonic() - began) * 1000
    title = sum(1 for pixel in area(24, 318, 120, 24) if (pixel >> 8 & 0xFF) > 200)
    clock = sum(1 for pixel in area(24, 408, 60, 14) if (pixel >> 8 & 0xFF) > 120)
    if index:
        values.append(took)
        incomplete += title < 20 or clock < 10
    time.sleep(0.3)
    stop(proc)
report("launch, by pixels on X", values, LIMIT_WARM)
results.append(incomplete == 0)
print(f"{'PASS' if incomplete == 0 else 'FAIL'}  first frame is already complete               {len(values) - incomplete}/{len(values)}")
print(f"{sum(results)}/{len(results)} passed")
sys.exit(0 if all(results) else 1)
