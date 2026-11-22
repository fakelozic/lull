"""Step 3 against real KDE parts on private, invisible servers: kded's tray watcher, Plasma's system-tray
widget, playerctl, and real clicks through XTEST. Run it with scripts/desktop-e2e.sh.
"""
import ctypes
import json
import os
import pathlib
import re
import shutil
import signal
import subprocess
import sys
import time

if not os.path.isdir("/sandbox/run") or os.path.exists("/dev/snd") or os.path.exists("/dev/dri"):
    sys.exit("refusing to run outside the sandboxes")

sys.path.insert(0, "/usr/lib64/python3/site-packages")
import gi  # noqa: E402

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402
import numpy as np  # noqa: E402

LULL = "/lull/target/release/lull"
OUT = pathlib.Path("/out")
STATE = pathlib.Path("/sandbox/state/lull")
RAIN = next(pathlib.Path("/sandbox/home/Music").glob("*.opus"))
NAME, PATH = "org.mpris.MediaPlayer2.lull", "/org/mpris/MediaPlayer2"
PLAYER = "org.mpris.MediaPlayer2.Player"
RATE = 48000
KDE_ENV = {key: value for key, value in os.environ.items() if key != "WAYLAND_DISPLAY"}
KDE_ENV.update(DISPLAY=":99", QT_QPA_PLATFORM="xcb", QT_QUICK_BACKEND="software", LIBGL_ALWAYS_SOFTWARE="1",
               __EGL_VENDOR_LIBRARY_FILENAMES="/usr/share/glvnd/egl_vendor.d/50_mesa.json")
APP_ENV = {key: value for key, value in KDE_ENV.items() if key != "QT_QPA_PLATFORM"}
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"{'PASS' if ok else 'FAIL'}  {name}  {detail}", flush=True)


def call(name, path, interface, method, args=None):
    return bus.call_sync(name, path, interface, method, args, None, Gio.DBusCallFlags.NONE, 4000, None).unpack()


def player(method, args=None):
    return call(NAME, PATH, PLAYER, method, args)


def prop(which, interface=PLAYER):
    return call(NAME, PATH, "org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", (interface, which)))[0]


def until(condition, seconds=6.0):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            if condition():
                return True
        except GLib.Error:
            pass
        time.sleep(0.03)
    return False


def owned(name=NAME):
    return call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner",
                GLib.Variant("(s)", (name,)))[0]


def log():
    lines = []
    for path in sorted((STATE / "logs").glob("lull-*.log")):
        lines += [json.loads(line) for line in path.read_text().splitlines()]
    return lines


def count(event, text=""):
    return sum(1 for line in log() if line["event"] == event and text in line["detail"])


def children(pid):
    found = []
    try:
        for task in pathlib.Path(f"/proc/{pid}/task").iterdir():
            found += [int(child) for child in (task / "children").read_text().split()]
    except OSError:
        pass
    return found


def lulls():
    found = subprocess.run(["pgrep", "-f", f"^({LULL}|lull)( |$)"], capture_output=True, text=True).stdout.split()
    return [int(pid) for pid in found if "--window" not in pathlib.Path(f"/proc/{pid}/cmdline").read_text()]


def pss(pid):
    try:
        return int(re.search(r"^Pss:\s+(\d+)", pathlib.Path(f"/proc/{pid}/smaps_rollup").read_text(), re.M)[1]) / 1024
    except OSError:
        return 0.0


def ticks(pid):
    fields = pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return int(fields[11]) + int(fields[12])


def cpu(pid, seconds):
    before = ticks(pid)
    time.sleep(seconds)
    return (ticks(pid) - before) / os.sysconf("SC_CLK_TCK") / seconds * 100


subprocess.Popen(["Xvfb", ":99", "-screen", "0", "1400x800x24", "-nolisten", "tcp", "-noreset", "-extension", "GLX"],
                 env=KDE_ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1.2)

xlib, xtst = ctypes.CDLL("libX11.so.6"), ctypes.CDLL("libXtst.so.6")
pointer, number = ctypes.c_void_p, ctypes.c_ulong
xlib.XOpenDisplay.restype = pointer
xlib.XDefaultRootWindow.argtypes, xlib.XDefaultRootWindow.restype = [pointer], number
xlib.XInternAtom.argtypes, xlib.XInternAtom.restype = [pointer, ctypes.c_char_p, ctypes.c_int], number
xlib.XQueryTree.argtypes = [pointer, number, ctypes.POINTER(number), ctypes.POINTER(number),
                            ctypes.POINTER(ctypes.POINTER(number)), ctypes.POINTER(ctypes.c_uint)]
xlib.XFetchName.argtypes = [pointer, number, ctypes.POINTER(ctypes.c_char_p)]
xlib.XMoveResizeWindow.argtypes = [pointer, number, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint]
xlib.XSetInputFocus.argtypes = [pointer, number, ctypes.c_int, number]
xlib.XSendEvent.argtypes = [pointer, number, ctypes.c_int, ctypes.c_long, pointer]
xlib.XSync.argtypes = [pointer, ctypes.c_int]
xtst.XTestFakeMotionEvent.argtypes = [pointer, ctypes.c_int, ctypes.c_int, ctypes.c_int, number]
xtst.XTestFakeButtonEvent.argtypes = [pointer, ctypes.c_uint, ctypes.c_int, number]
ignore_errors = ctypes.CFUNCTYPE(ctypes.c_int, pointer, pointer)(lambda display, event: 0)
xlib.XSetErrorHandler(ignore_errors)  # Windows may vanish while we look at them.
display = xlib.XOpenDisplay(b":99")


class ClientMessage(ctypes.Structure):
    _fields_ = [("type", ctypes.c_int), ("serial", number), ("send_event", ctypes.c_int), ("display", pointer),
                ("window", number), ("message_type", number), ("format", ctypes.c_int), ("data", ctypes.c_long * 5),
                ("pad", ctypes.c_long * 16)]


def x_windows(title):
    root, parent, kids, amount = number(), number(), ctypes.POINTER(number)(), ctypes.c_uint()
    xlib.XQueryTree(display, xlib.XDefaultRootWindow(display), ctypes.byref(root), ctypes.byref(parent),
                    ctypes.byref(kids), ctypes.byref(amount))
    found = []
    for index in range(amount.value):
        name = ctypes.c_char_p()
        if xlib.XFetchName(display, kids[index], ctypes.byref(name)) and name.value and title in name.value:
            found.append(kids[index])
    return found


def place(title, x, y, width, height):
    for window in x_windows(title):
        xlib.XMoveResizeWindow(display, window, x, y, width, height)
    xlib.XSync(display, 0)


def focus_lull():
    for window in x_windows(b"Lull"):
        xlib.XSetInputFocus(display, window, 1, 0)
    xlib.XSync(display, 0)


def close_lull():
    """What a click on the window's close button sends."""
    for window in x_windows(b"Lull"):
        message = ClientMessage(type=33, send_event=1, display=display, window=window, format=32)
        message.message_type = xlib.XInternAtom(display, b"WM_PROTOCOLS", 0)
        message.data[0] = xlib.XInternAtom(display, b"WM_DELETE_WINDOW", 0)
        xlib.XSendEvent(display, window, 0, 0, ctypes.byref(message))
    xlib.XSync(display, 0)


def click(spot, button=1):
    xtst.XTestFakeMotionEvent(display, 0, spot[0], spot[1], 0)
    xlib.XSync(display, 0)
    time.sleep(0.25)
    xtst.XTestFakeButtonEvent(display, button, 1, 0)
    xtst.XTestFakeButtonEvent(display, button, 0, 0)
    xlib.XSync(display, 0)


def screenshot(name=None):
    done = subprocess.run(["import", "-silent", "-window", "root", "-depth", "8", "ppm:-"], env=APP_ENV,
                          capture_output=True, timeout=10, check=True)
    header = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", done.stdout)
    if name:
        (OUT / f"{name}.ppm").write_bytes(done.stdout)
    width, height = int(header[1]), int(header[2])
    return np.frombuffer(done.stdout, np.uint8, width * height * 3, header.end()).reshape(height, width, 3)


def start_kded():
    return subprocess.Popen(["kded6"], env=KDE_ENV, stdout=subprocess.DEVNULL, stderr=open(OUT / "kded.log", "a"))


# Only the tray watcher: the other modules start things like Plasma's welcome screen.
config = pathlib.Path("/sandbox/home/.config")
config.mkdir(parents=True, exist_ok=True)
modules = [path.stem for path in pathlib.Path("/usr/lib64/qt6/plugins/kf6/kded").glob("*.so")]
(config / "kded6rc").write_text("".join(
    f"[Module-{module}]\nautoload=false\n" for module in modules if module != "statusnotifierwatcher"))
kded = start_kded()
time.sleep(2.5)
widget = subprocess.Popen(["plasmawindowed", "org.kde.plasma.systemtray"], env=KDE_ENV, stdout=subprocess.DEVNULL,
                          stderr=open(OUT / "plasmawindowed.log", "w"))
time.sleep(6)
TRAY_AT = (500, 0, 640, 64)


def place_tray():
    root, parent, kids, amount = number(), number(), ctypes.POINTER(number)(), ctypes.c_uint()
    xlib.XQueryTree(display, xlib.XDefaultRootWindow(display), ctypes.byref(root), ctypes.byref(parent),
                    ctypes.byref(kids), ctypes.byref(amount))
    for index in range(amount.value):
        name = ctypes.c_char_p()
        if xlib.XFetchName(display, kids[index], ctypes.byref(name)) and name.value and b"Lull" not in name.value:
            xlib.XMoveResizeWindow(display, kids[index], *TRAY_AT)
    xlib.XSync(display, 0)


place_tray()
time.sleep(2)
before_icon = screenshot("tray-empty")

STATE.mkdir(parents=True, exist_ok=True)
(STATE / "state").write_bytes(f"lull-state 1\nposition={2840 * RATE}\nvolume=45\ntrack={RAIN}".encode())
proc = subprocess.Popen([LULL], env=APP_ENV, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                        stderr=open(OUT / "lull.log", "w"))
check("starts, with a bus name and a tray icon", until(lambda: owned() and count("window.opened")) and count("tray.started", "online=true"))
items = call("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.freedesktop.DBus.Properties", "Get",
             GLib.Variant("(ss)", ("org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems")))[0]
check("registered with KDE's real tray service", len(items) == 1, str(items))
time.sleep(2.5)
with_icon = screenshot("tray-paused")
x0, y0, width, height = TRAY_AT
changed = np.argwhere(np.abs(with_icon[y0:y0 + height, x0:x0 + width].astype(int)
                             - before_icon[y0:y0 + height, x0:x0 + width]).sum(2) > 60)
check("the icon shows in Plasma's own tray widget", len(changed) > 20, f"{len(changed)} px")
# Plasma's media controller shows an icon for Lull as well, with the same middle-click and scroll. So aim by
# the ring's own colour, and take only Lull's click log as proof that it was hit.
ours = np.argwhere(np.abs(with_icon[y0:y0 + height, x0:x0 + width].astype(int) - (129, 139, 121)).max(2) <= 4)
check("the paused ring is the icon being aimed at", len(ours) > 8, f"{len(ours)} px")
ICON = (x0 + int((ours[:, 1].min() + ours[:, 1].max()) / 2),
        y0 + int((ours[:, 0].min() + ours[:, 0].max()) / 2)) if len(ours) else (x0 + 20, y0 + 20)
OUTSIDE = (1200, 700)

# Tray
click(ICON, 2)
check("middle-click plays", until(lambda: prop("PlaybackStatus") == "Playing"))
time.sleep(1.5)
screenshot("tray-playing")
click(ICON, 2)
check("middle-click again pauses", until(lambda: prop("PlaybackStatus") == "Paused"))
click(ICON, 4)
check("scroll up is louder", until(lambda: abs(prop("Volume") - 0.50) < 0.001), str(prop("Volume")))
click(ICON, 5)
check("scroll down is quieter", until(lambda: abs(prop("Volume") - 0.45) < 0.001), str(prop("Volume")))

focus_lull()
time.sleep(0.5)
click(ICON)
check("click hides a focused window", until(lambda: not children(proc.pid)) and proc.poll() is None)
time.sleep(0.4)  # Clicks closer than 300 ms count as one, so a double-click doesn't show and hide.
started = time.monotonic()
opened = count("window.opened")
click(ICON)
check("click again brings it back", until(lambda: count("window.opened") > opened and len(children(proc.pid)) == 1),
      f"{(time.monotonic() - started) * 1000:.0f} ms including the 250 ms test pointer pause")
xlib.XSetInputFocus(display, 0, 0, 0)
xlib.XSync(display, 0)
time.sleep(0.6)
old = children(proc.pid)
click(ICON)
check("the clicks really hit Lull's icon", until(lambda: count("tray.clicked") >= 3), str(count("tray.clicked")))
check("click on a window behind others raises it",
      until(lambda: len(children(proc.pid)) == 1 and children(proc.pid) != old), f"{old} to {children(proc.pid)}")

# Media controls
check("playerctl sees it", "with thunder" in subprocess.run(
    ["playerctl", "-p", "lull", "metadata"], capture_output=True, text=True, timeout=5).stdout)
subprocess.run(["playerctl", "-p", "lull", "play"], timeout=5)
check("media key: play", until(lambda: prop("PlaybackStatus") == "Playing"))
time.sleep(1.5)
seeked, changes = [], []
bus.signal_subscribe(None, PLAYER, "Seeked", PATH, None, 0, lambda *args: seeked.append(args[5].unpack()[0]))
bus.signal_subscribe(None, "org.freedesktop.DBus.Properties", "PropertiesChanged", PATH, None, 0,
                     lambda *args: changes.append(args[5].unpack()[1]))


def pump(seconds):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        while GLib.MainContext.default().iteration(False):
            pass
        time.sleep(0.05)


before = prop("Position")
player("Seek", GLib.Variant("(x)", (60_000_000,)))
pump(1.2)
check("Seek moves 60 s", abs(prop("Position") - before - 61_200_000) < 2_000_000, f"{(prop('Position') - before) / 1e6:.1f} s")
check("and says so with Seeked", len(seeked) >= 1, str(len(seeked)))
track = prop("Metadata")["mpris:trackid"]
player("SetPosition", GLib.Variant("(ox)", (track, 1_000_000_000)))
pump(0.8)
check("SetPosition goes there", abs(prop("Position") - 1_000_800_000) < 2_000_000, f"{prop('Position') / 1e6:.1f} s")
player("SetPosition", GLib.Variant("(ox)", ("/local/lull/track/99", 5_000_000)))
time.sleep(0.6)
check("SetPosition for a stale track is ignored", prop("Position") > 999_000_000)
seeked.clear()
pump(4)
check("no Seeked during ordinary playing", not seeked, str(seeked))
call(NAME, PATH, "org.freedesktop.DBus.Properties", "Set", GLib.Variant("(ssv)", (PLAYER, "Volume", GLib.Variant("d", 7.0))))
pump(1)
check("Volume clamps at 100%", prop("Volume") == 1.0)
check("and announces the change", any("Volume" in change for change in changes))
call(NAME, PATH, "org.freedesktop.DBus.Properties", "Set", GLib.Variant("(ssv)", (PLAYER, "Volume", GLib.Variant("d", 0.45))))
for uri in ("file:///etc/passwd", "file:///sandbox/home/Music/../../../etc/passwd", "http://example.invalid/a.opus"):
    try:
        player("OpenUri", GLib.Variant("(s)", (uri,)))
        refused = False
    except GLib.Error as error:
        refused = "AccessDenied" in str(error)
    check("OpenUri refuses anything outside ~/Music", refused, uri)
check("still playing after all that", prop("PlaybackStatus") == "Playing")

# Closing
paused_before, connected_before = count("audio.paused"), count("audio.connected")
time.sleep(0.5)
close_lull()
check("closing leaves it in the tray", until(lambda: not children(proc.pid)) and proc.poll() is None)
time.sleep(1.5)
check("closing doesn't touch the sound", prop("PlaybackStatus") == "Playing"
      and (count("audio.paused"), count("audio.connected")) == (paused_before, connected_before))
check("the first close says so", until(lambda: (STATE / "tray-notice-shown").exists()) and not count("tray.notice_failed"))
time.sleep(1)
screenshot("tray-notice")
playing_cpu, playing_mb = cpu(proc.pid, 20), pss(proc.pid)
check("hidden and playing: 15 MB or less, under 3% CPU", playing_mb <= 15 and playing_cpu < 3, f"{playing_mb:.1f} MB, {playing_cpu:.2f}%")
player("Pause")
time.sleep(2.5)
paused_cpu, paused_mb = cpu(proc.pid, 20), pss(proc.pid)
check("hidden and paused: 15 MB or less, under 0.3% CPU", paused_mb <= 15 and paused_cpu < 0.3, f"{paused_mb:.1f} MB, {paused_cpu:.2f}%")

# A second launch
opened = count("window.opened")
started = time.monotonic()
second = subprocess.run([LULL], env=APP_ENV, capture_output=True, text=True, timeout=8)
took = (time.monotonic() - started) * 1000
check("a second launch exits at once", second.returncode == 0 and took < 100, f"{took:.0f} ms")
check("and brings the window back", until(lambda: count("window.opened") > opened) and len(lulls()) == 1)
subprocess.run([LULL, "--play", "--at", "1:00:00"], env=APP_ENV, capture_output=True, timeout=8)
check("a second launch passes --play and --at",
      until(lambda: prop("PlaybackStatus") == "Playing" and abs(prop("Position") - 3_601_000_000) < 4_000_000))
refused = subprocess.run([LULL, "/lull/tests/fixtures/noise.opus"], env=APP_ENV, capture_output=True, text=True, timeout=8)
check("a second launch can't open files outside ~/Music", refused.returncode == 2 and "~/Music" in refused.stderr)

# A flood from another program
peak = 0.0
flood = [subprocess.Popen(["/usr/bin/python3", "-c",
                           "import gi\nfrom gi.repository import Gio, GLib\n"
                           "b = Gio.bus_get_sync(Gio.BusType.SESSION, None)\n"
                           f"for i in range(6000):\n    b.call('{NAME}', '{PATH}', '{PLAYER}', 'Seek', "
                           "GLib.Variant('(x)', (1000000,)), None, 0, -1, None, None)\n"
                           "b.flush_sync(None)\n"
                           "import time\ntime.sleep(3)\n"],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) for _ in range(4)]
deadline = time.monotonic() + 25
while time.monotonic() < deadline and any(process.poll() is None for process in flood):
    peak = max(peak, pss(proc.pid) + sum(pss(child) for child in children(proc.pid)))
    time.sleep(0.1)
for process in flood:
    process.kill()
check("24,000 calls at once stay under 50 MB", 0 < peak < 50, f"peak {peak:.1f} MB")
check("and it still answers", until(lambda: prop("PlaybackStatus") == "Playing", 8) and proc.poll() is None)

# Big messages, at both of Lull's connections (its own, and the tray library's)
targets = ((NAME, PATH, "local.lull.Control1"),
           (f"org.kde.StatusNotifierItem-{proc.pid}-1", "/StatusNotifierItem", "org.kde.StatusNotifierItem"))
(OUT / "big.py").write_text(
    "import sys, time\nfrom gi.repository import Gio, GLib\n"
    "name, path, interface, size, many = sys.argv[1:6]\n"
    "bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)\n"
    "blob = GLib.Variant('(s)', ('x' * int(size),))\n"
    "for i in range(int(many)):\n"
    "    bus.call(name, path, interface, 'ProvideXdgActivationToken', blob, None, 0, 2000, None, None)\n"
    "    bus.flush_sync(None)\n"
    "time.sleep(1)\n")
family = lambda: pss(proc.pid) + sum(pss(child) for child in children(proc.pid))
for name, path, interface in targets:
    for size, many in ((16 << 20, 3), (120_000, 800), (30_000, 4000), (300, 30_000)):
        sender = subprocess.Popen(["/usr/bin/python3", str(OUT / "big.py"), name, path, interface, str(size), str(many)],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        peak, deadline = 0.0, time.monotonic() + 25
        while time.monotonic() < deadline and sender.poll() is None:
            peak = max(peak, family())
            time.sleep(0.02)
        sender.kill()
        check(f"{many} messages of {size} bytes at {interface.split('.')[-1]} stay under 50 MB", 0 < peak < 50,
              f"peak {peak:.1f} MB")
check("both connections outlive oversized messages", until(lambda: prop("PlaybackStatus") == "Playing", 8)
      and until(lambda: call(targets[1][0], "/StatusNotifierItem", "org.freedesktop.DBus.Properties", "Get",
                             GLib.Variant("(ss)", ("org.kde.StatusNotifierItem", "Id")))[0] == "lull", 8))

# Every kind of call, flat out, including the ones that start window processes
item = f"org.kde.StatusNotifierItem-{proc.pid}-1"
(OUT / "every.py").write_text(
    "import sys, time\nfrom gi.repository import Gio, GLib\n"
    "which, name = sys.argv[1], sys.argv[2]\n"
    "bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)\n"
    "V = GLib.Variant\n"
    "calls = {\n"
    " 'Raise': ('/org/mpris/MediaPlayer2', 'org.mpris.MediaPlayer2', lambda: None),\n"
    " 'Show': ('/org/mpris/MediaPlayer2', 'local.lull.Control1', lambda: V('(sbxs)', ('t' * 256, False, -1, ''))),\n"
    " 'Activate': ('/StatusNotifierItem', 'org.kde.StatusNotifierItem', lambda: V('(ii)', (0, 0))),\n"
    " 'Scroll': ('/StatusNotifierItem', 'org.kde.StatusNotifierItem', lambda: V('(is)', (2147483647, 'vertical'))),\n"
    " 'GetLayout': ('/MenuBar', 'com.canonical.dbusmenu', lambda: V('(iias)', (0, -1, ['p' * 50] * 100))),\n"
    " 'GetAll': ('/org/mpris/MediaPlayer2', 'org.freedesktop.DBus.Properties', lambda: V('(s)', ('org.mpris.MediaPlayer2.Player',))),\n"
    " 'OpenUri': ('/org/mpris/MediaPlayer2', 'org.mpris.MediaPlayer2.Player', lambda: V('(s)', ('file:///' + 'a/' * 6000,)))}\n"
    "path, interface, body = calls[which]\n"
    "end = time.monotonic() + 8\n"
    "while time.monotonic() < end:\n"
    "    bus.call(name, path, interface, which, body(), None, 0, 2000, None, None)\n"
    "    bus.flush_sync(None)\n"
    "    time.sleep(0.0005)\n")
for which in ("Raise", "Show", "Activate", "Scroll", "GetLayout", "GetAll", "OpenUri"):
    name = item if which in ("Activate", "Scroll", "GetLayout") else NAME
    sender = subprocess.Popen(["/usr/bin/python3", str(OUT / "every.py"), which, name],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    peak, deadline = 0.0, time.monotonic() + 20
    while time.monotonic() < deadline and sender.poll() is None:
        peak = max(peak, family())
        time.sleep(0.01)
    sender.kill()
    time.sleep(2)
    check(f"a flood of {which} stays under 50 MB", 0 < peak < 50 and proc.poll() is None,
          f"peak {peak:.1f} MB, {len(children(proc.pid))} window processes after")
check("and it still answers after every flood", until(lambda: prop("PlaybackStatus") in ("Playing", "Paused"), 8))

# The terminal it was started from closing
proc.send_signal(signal.SIGHUP)
time.sleep(2)
check("closing the terminal doesn't quit a tray app", proc.poll() is None and owned())

# Plasma restarting
check("floods can't fill the log", sum(path.stat().st_size for path in (STATE / "logs").glob("*.log")) < 250_000,
      f"{sum(path.stat().st_size for path in (STATE / 'logs').glob('*.log')) // 1024} KB")
kded.terminate()
kded.wait(8)
check("notices when the tray goes away", until(lambda: count("tray.offline") > 0, 8))
kded = start_kded()
check("and comes back with it", until(lambda: count("tray.online") > 0, 15))
time.sleep(2)

# Quitting, and launching again during the fade
call(NAME, PATH, "org.mpris.MediaPlayer2", "Quit")
again = subprocess.Popen([LULL], env=APP_ENV, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=open(OUT / "lull-again.log", "w"))
try:
    code = proc.wait(8)
except subprocess.TimeoutExpired:
    code = "hung"
check("Quit fades out and exits cleanly", code == 0 and count("shutdown.done") >= 1, str(code))
check("launching during the fade starts a fresh one", until(lambda: again.poll() is None and owned() and lulls() == [again.pid], 8))
again.send_signal(signal.SIGTERM)
again.wait(8)
time.sleep(1)

# Ctrl+Q in the window
xtst.XTestFakeKeyEvent.argtypes = [pointer, ctypes.c_uint, ctypes.c_int, number]
xlib.XKeysymToKeycode.argtypes = [pointer, number]
keyboard = subprocess.Popen([LULL], env=APP_ENV, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
until(lambda: owned() and children(keyboard.pid))
time.sleep(2)
xtst.XTestFakeMotionEvent(display, 0, 180, 160, 0)
focus_lull()
control, letter = xlib.XKeysymToKeycode(display, 0xFFE3), xlib.XKeysymToKeycode(display, ord("q"))
for code, down in ((control, 1), (letter, 1), (letter, 0), (control, 0)):
    xtst.XTestFakeKeyEvent(display, code, down, 0)
xlib.XSync(display, 0)
try:
    code = keyboard.wait(6)
except subprocess.TimeoutExpired:
    keyboard.kill()
    code = "still running"
check("Ctrl+Q really quits", code == 0, str(code))
time.sleep(1)

# Five launches at the same moment
racers = [subprocess.Popen([LULL], env=APP_ENV, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL) for _ in range(5)]
time.sleep(5)
running = [racer for racer in racers if racer.poll() is None]
check("five launches at once make one Lull", len(running) == 1 and all(r.returncode == 0 for r in racers if r not in running),
      str([racer.returncode for racer in racers]))
for racer in running:
    racer.send_signal(signal.SIGTERM)
    racer.wait(8)
time.sleep(1)

# The tray menu's Quit
menu_run = subprocess.Popen([LULL], env=APP_ENV, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
until(owned)
time.sleep(1.5)
item = f"org.kde.StatusNotifierItem-{menu_run.pid}-1"
layout = call(item, "/MenuBar", "com.canonical.dbusmenu", "GetLayout", GLib.Variant("(iias)", (0, -1, [])))[1][2]
labels = {entry[1].get("label"): entry[0] for entry in layout}
check("the menu has Play, Open and Quit", {"Play", "Open Lull", "Quit"} <= set(labels), str(list(labels)))
click(ICON, 3)
time.sleep(2)
screenshot("tray-menu")
call(item, "/MenuBar", "com.canonical.dbusmenu", "Event", GLib.Variant("(isvu)", (labels["Quit"], "clicked", GLib.Variant("s", ""), 0)))
try:
    code = menu_run.wait(8)
except subprocess.TimeoutExpired:
    menu_run.kill()
    code = "hung"
check("Quit in the menu works", code == 0)
check("nothing went wrong in the log", not [line for line in log() if line["level"] == "error"
                                            and "~/Music while it runs" not in line["detail"]
                                            and "Quit it first" not in line["detail"]],
      str([line["event"] for line in log() if line["level"] == "error"]))
shutil.rmtree(OUT / "logs", ignore_errors=True)
shutil.copytree(STATE / "logs", OUT / "logs")
widget.terminate()
kded.terminate()
print(f"{sum(results)}/{len(results)} passed")
sys.exit(0 if all(results) else 1)
