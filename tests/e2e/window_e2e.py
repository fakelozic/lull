"""End-to-end checks of both Lull processes, on private, invisible servers.

Three private displays, each for what it can be trusted with:
- headless Mutter: CPU, RAM, hiding. It really draws, so frame callbacks flow.
- virtual KWin: the close request only. Without a GPU it does no compositing and shows one frame,
  so any CPU figure from it leaves out drawing.
- Xvfb: pictures and video. KWin's ScreenShot2 segfaults without compositing.
Run it with scripts/window-e2e.sh.
"""
import json
import os
import pathlib
import re
import signal
import subprocess
import sys
import threading
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
STATE = pathlib.Path("/sandbox/state/lull/state")
RATE = 48000
TICKS = os.sysconf("SC_CLK_TCK")
WIDTH, HEIGHT = 360, 540
X_ENV = {key: value for key, value in os.environ.items() if key != "WAYLAND_DISPLAY"}
X_ENV.update(DISPLAY=":99", __EGL_VENDOR_LIBRARY_FILENAMES="/usr/share/glvnd/egl_vendor.d/50_mesa.json")
DRAWN = {**os.environ, "WAYLAND_DISPLAY": "lull-mutter"}
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
results = []


def check(name, ok, detail=""):
    results.append(ok)
    print(f"{'PASS' if ok else 'FAIL'}  {name}  {detail}", flush=True)


def fresh(volume=45, position=2840 * RATE):
    STATE.parent.mkdir(parents=True, exist_ok=True)
    track = "/sandbox/home/Music/Rain on Window with Thunder (8h).opus"
    STATE.write_bytes(f"lull-state 1\nposition={position}\nvolume={volume}\ntrack={track}".encode())


def saved(field="position"):
    return int(re.search(rf"{field}=(\d+)".encode(), STATE.read_bytes())[1])


def screenshot(name=None):
    """Lull's window on the private X screen, as RGB rows."""
    done = subprocess.run(["import", "-silent", "-window", "root", "-depth", "8", "ppm:-"], env=X_ENV,
                          capture_output=True, timeout=10, check=True)
    header = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", done.stdout)
    width, height = int(header[1]), int(header[2])
    pixels = np.frombuffer(done.stdout, np.uint8, width * height * 3, header.end()).reshape(height, width, 3)
    if name:
        (OUT / f"{name}.ppm").write_bytes(f"P6\n{width} {height}\n255\n".encode() + pixels.tobytes())
    return pixels


def same(first, second, box=(0, 0, WIDTH, HEIGHT)):
    x0, y0, x1, y1 = box
    return np.array_equal(first[y0:y1, x0:x1], second[y0:y1, x0:x1])


def log_lines():
    lines = []
    for path in sorted((STATE.parent / "logs").glob("lull-*.log")):
        lines += [json.loads(line) for line in path.read_text().splitlines()]
    return lines


def kwin_script(source):
    path = OUT / "action.js"
    path.write_text(source)
    call = lambda method, args: bus.call_sync("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting", method, args,
                                              None, Gio.DBusCallFlags.NONE, 5000, None)
    call("unloadScript", GLib.Variant("(s)", ("lull-test",)))
    call("loadScript", GLib.Variant("(ss)", (str(path), "lull-test")))
    call("start", None)


def on_window(action):
    kwin_script('for (const w of workspace.windowList()) if (String(w.caption).startsWith("Lull")) { %s }' % action)


def children(pid):
    found = []
    for task in pathlib.Path(f"/proc/{pid}/task").iterdir():
        found += [int(child) for child in (task / "children").read_text().split()]
    return found


def ticks(pid):
    fields = pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return int(fields[11]) + int(fields[12])


def pss(pid):
    return int(re.search(r"^Pss:\s+(\d+)", pathlib.Path(f"/proc/{pid}/smaps_rollup").read_text(), re.M)[1]) / 1024


def alive(pid):
    try:
        return pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except OSError:
        return False


class Lull:
    count = 0

    def __init__(self, *args, env=None):
        Lull.count += 1
        self.log = OUT / f"window-{Lull.count}.log"
        self.file = open(self.log, "w")
        self.proc = subprocess.Popen([LULL, *args], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=self.file, env=env)

    def wait_for(self, text, seconds=10.0, times=1):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if self.log.read_text().count(text) >= times:
                return True
            time.sleep(0.05)
        return False

    def window(self):
        found = children(self.proc.pid)
        return found[0] if found else None

    def cost(self, seconds):
        window = self.window()
        before = ticks(self.proc.pid), ticks(window)
        time.sleep(seconds)
        sound, shown = [(now - then) / TICKS / seconds * 100
                        for now, then in zip((ticks(self.proc.pid), ticks(window)), before)]
        return sound, shown, pss(self.proc.pid), pss(window)

    def said(self, event, text=""):
        return [line for line in log_lines() if line["event"] == event and text in line["detail"]
                and line["pid"] in (self.proc.pid, self.window())]

    def stop(self):
        self.proc.send_signal(signal.SIGTERM)
        code = self.proc.wait(10)
        self.file.close()
        return code


def opens():
    fresh()
    started = time.monotonic()
    lull = Lull(env=DRAWN)
    opened = lull.wait_for("window.opened")
    took = time.monotonic() - started
    check("the window opens on Wayland", opened, f"{took * 1000:.0f} ms")
    time.sleep(3)
    sound, shown, sound_mb, shown_mb = lull.cost(20)
    check("paused: 40 MB or less, under 0.5% CPU", sound_mb + shown_mb <= 40 and sound + shown < 0.5,
          f"sound {sound_mb:.1f} MB {sound:.2f}%, window {shown_mb:.1f} MB {shown:.2f}%")
    check("quit is clean", lull.stop() == 0)


def playing():
    fresh()
    lull = Lull("--play", env=DRAWN)
    lull.wait_for("window.opened")
    time.sleep(8)
    sound, shown, sound_mb, shown_mb = lull.cost(60)
    check("a visible window never counts as hidden", not lull.said("window.visible", "visible=false"))
    check("the ring is really being drawn", shown > 0.5, f"window {shown:.2f}%")
    check("playing: 40 MB or less", sound_mb + shown_mb <= 40,
          f"sound {sound_mb:.1f} MB + window {shown_mb:.1f} MB = {sound_mb + shown_mb:.1f} MB")
    check("playing: under 5% of a core", sound + shown < 5, f"sound {sound:.2f}% + window {shown:.2f}% = {sound + shown:.2f}%")
    moved = saved() / RATE - 2840
    check("the 30 s save keeps running while playing", moved >= 25, f"saved {moved:.1f} s in")

    # A compositor that stops answering frames is what a minimised window sees.
    os.kill(mutter.pid, signal.SIGSTOP)
    try:
        time.sleep(4)
        heard, hidden, _, _ = lull.cost(15)
    finally:
        os.kill(mutter.pid, signal.SIGCONT)
    check("unseen: the ring stops drawing", hidden < 0.5, f"window {hidden:.2f}%, sound {heard:.2f}%")
    time.sleep(3)
    _, back, _, _ = lull.cost(10)
    check("seen again: the ring runs again", back > 0.5 and bool(lull.said("window.visible", "visible=true")), f"window {back:.2f}%")
    code = lull.stop()
    check("no underrun with the window open", code == 0 and "audio.underrun" not in lull.log.read_text())


def crash():
    fresh()
    lull = Lull("--play", env=DRAWN)
    lull.wait_for("window.opened")
    lull.wait_for("audio.connected")
    time.sleep(2)
    old = lull.window()
    os.kill(old, signal.SIGKILL)
    back = lull.wait_for("window.opened", times=2)
    time.sleep(2)
    text = lull.log.read_text()
    check("a window crash restarts the window", back and lull.window() not in (None, old))
    check("a window crash doesn't touch the sound",
          text.count("audio.connected") == 1 and not any(word in text for word in ("underrun", "paused", "lost")))
    window = lull.window()
    os.kill(window, signal.SIGSTOP)
    time.sleep(6)
    still = lull.proc.poll() is None and "audio.underrun" not in lull.log.read_text()
    started = time.monotonic()
    code = lull.stop()
    check("a frozen window can't stall sound or quit", still and code == 0 and time.monotonic() - started < 5)
    log = log_lines()
    crashed = [line["detail"] for line in log if line["event"] == "window.crashed"]
    check("the log says how the window died", any("signal: 9" in detail for detail in crashed), "; ".join(crashed))
    check("both processes share one log", {"sound", "window"} <= {line["process"] for line in log})


def closing():
    fresh()
    lull = Lull("--play")
    lull.wait_for("window.opened")
    time.sleep(3)
    window = lull.window()
    started = time.monotonic()
    on_window("w.closeWindow();")
    try:
        code = lull.proc.wait(8)
    except subprocess.TimeoutExpired:
        code = None
        lull.stop()
    took = time.monotonic() - started
    moved = saved() / RATE - 2840
    check("closing fades out, saves and quits", code == 0 and took < 3 and moved > 2, f"{took:.2f} s, saved {moved:.1f} s in")
    check("no window process is left", not alive(window))


def sound_killed():
    fresh()
    lull = Lull("--play")
    lull.wait_for("window.opened")
    time.sleep(1)
    window = lull.window()
    lull.proc.kill()
    lull.proc.wait()
    time.sleep(2)
    check("the window exits if the sound process dies", not alive(window))


def no_display():
    fresh()
    env = {key: value for key, value in os.environ.items() if key not in ("WAYLAND_DISPLAY", "DISPLAY")}
    done = subprocess.run([LULL], env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=20)
    check("no display gives a plain error, not a hang", done.returncode == 1 and "--no-window" in done.stderr)


def pictures():
    fresh()
    lull = Lull(env=X_ENV)
    lull.wait_for("window.opened")
    time.sleep(2)
    paused = screenshot("real-paused")
    found = np.argwhere((paused == (9, 13, 10)).all(2))
    size = tuple(int(n) + 1 for n in found.max(0)[::-1]) if len(found) else (0, 0)
    check("the window is exactly 360x540", size == (WIDTH, HEIGHT), str(size))
    time.sleep(1.5)
    check("paused: nothing on screen changes", same(paused, screenshot()))
    lull.stop()

    fresh()
    lull = Lull("--play", env=X_ENV)
    lull.wait_for("window.opened")
    time.sleep(3)
    first = screenshot()
    time.sleep(3.3)
    later = screenshot("real-playing")
    check("playing: the ring moves", not same(first, later, (60, 60, 300, 290)))
    check("playing: the clock moves", not same(first, later, (24, 408, 90, 424)))
    lull.stop()


def frame_rate():
    """The minimum is 30 fps. Samples the screen faster than that and counts new pictures."""
    fresh()
    lull = Lull("--play", env=X_ENV)
    lull.wait_for("window.opened")
    time.sleep(2)
    seconds = 10
    grab = subprocess.run(
        ["ffmpeg", "-nostdin", "-v", "error", "-f", "x11grab", "-draw_mouse", "0", "-framerate", "120", "-video_size", "300x270",
         "-i", ":99+30,36", "-t", str(seconds), "-pix_fmt", "gray", "-f", "rawvideo", "-"],
        env=X_ENV, capture_output=True, timeout=40, check=True).stdout
    lull.stop()
    pictures = np.frombuffer(grab, np.uint8).reshape(-1, 300 * 270)
    fresh_pictures = int((pictures[1:] != pictures[:-1]).any(1).sum())
    check("the ring runs at 30 fps", fresh_pictures / seconds >= 29.5, f"{fresh_pictures / seconds:.1f} fps")


def flashing():
    """The flash limit (WCAG general flash), on the real window, across the thunder at 47:26."""
    fresh(position=2836 * RATE)
    lull = Lull("--play", env=X_ENV)
    lull.wait_for("window.opened")
    frames, fps = 25 * 22, 25
    grab = subprocess.run(
        ["ffmpeg", "-nostdin", "-v", "error", "-f", "x11grab", "-draw_mouse", "0", "-framerate", str(fps),
         "-video_size", f"{WIDTH}x{HEIGHT}", "-i", ":99", "-frames:v", str(frames), "-pix_fmt", "rgb24", "-f", "rawvideo", "-"],
        env=X_ENV, capture_output=True, timeout=60, check=True).stdout
    lull.stop()
    video = np.frombuffer(grab, np.uint8).reshape(-1, HEIGHT, WIDTH, 3)
    scale = np.arange(256) / 255
    linear = np.where(scale <= 0.04045, scale / 12.92, ((scale + 0.055) / 1.055) ** 2.4).astype(np.float32)
    extreme = direction = None
    history = []
    area = 0
    for frame in video:
        light = linear[frame] @ np.array([0.2126, 0.7152, 0.0722], np.float32)
        if extreme is None:
            extreme, direction = light.copy(), np.zeros(light.shape, np.int8)
            continue
        further = ((direction > 0) & (light > extreme)) | ((direction < 0) & (light < extreme))
        extreme[further] = light[further]
        change = light - extreme
        turned = (np.abs(change) >= 0.1) & (np.minimum(light, extreme) < 0.8) & (np.sign(change) != direction)
        direction[turned] = np.sign(change[turned])
        extreme[turned] = light[turned]
        history = (history + [turned])[-fps:]
        area = max(area, int((np.sum(history, axis=0) > 6).sum()))
    check("flash check on the real window", area < 21824, f"{area} px of 21824 allowed, over {len(video)} frames")


def in_step_with_sound():
    """A burst of bass after steady noise must reach the ring when it reaches the speakers."""
    clip = "/tmp/burst.opus"
    subprocess.run(
        ["ffmpeg", "-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i",
         "aevalsrc='0.012*(random(0)-0.5)+if(gte(t,8),0.6*sin(2*PI*70*t),0)':s=48000:d=16", "-ac", "2", "-c:a", "libopus",
         "-b:a", "128k", clip], timeout=30, check=True)
    fresh(volume=100)
    lull = Lull("--play", "--at", "3", clip, env=X_ENV)
    lull.wait_for("window.opened")
    time.sleep(1)
    heard = []
    seen = []
    listen = subprocess.Popen(["parec", "--device=test_a.monitor", "--format=float32le", "--rate=48000", "--channels=2",
                               "--latency-msec=10", "--raw"], stdout=subprocess.PIPE)
    watch = subprocess.Popen(
        ["ffmpeg", "-nostdin", "-v", "error", "-f", "x11grab", "-draw_mouse", "0", "-framerate", "50", "-video_size",
         "300x300", "-i", ":99+30,18", "-pix_fmt", "gray", "-f", "rawvideo", "-"], stdout=subprocess.PIPE, env=X_ENV)

    def hear():
        while data := listen.stdout.read(480 * 8):
            samples = np.frombuffer(data, np.float32)
            heard.append((time.monotonic(), float(np.abs(samples).max()) if len(samples) else 0.0))

    def see():
        while len(data := watch.stdout.read(90000)) == 90000:
            image = np.frombuffer(data, np.uint8).reshape(300, 300).astype(np.float32)
            # The inner circle's size: how far out its brightest pixel sits, over four directions.
            lines = (image[149, 150:232], image[150, 149:67:-1], image[150:232, 149], image[149:67:-1, 150])
            seen.append((time.monotonic(), float(np.mean([np.argmax(line) for line in lines]))))

    threads = [threading.Thread(target=hear), threading.Thread(target=see)]
    for thread in threads:
        thread.start()
    time.sleep(7)
    listen.terminate()
    watch.terminate()
    for thread in threads:
        thread.join(5)
    lull.stop()
    sound_at = next((at for at, peak in heard if peak > 0.1), None)
    before = [size for at, size in seen if sound_at and sound_at - 2 < at < sound_at - 0.6]
    limit = max(before) + 1.5 if before else float("inf")
    ring_at = next((at for at, size in seen if sound_at and at > sound_at - 0.6 and size > limit), None)
    gap = ring_at - sound_at if ring_at else float("inf")
    check("the ring moves with what is heard", -0.1 <= gap <= 0.3, f"ring {gap * 1000:+.0f} ms against the speakers")


subprocess.Popen(["Xvfb", ":99", "-screen", "0", "800x700x24", "-nolisten", "tcp", "-noreset", "-extension", "GLX"],
                 env=X_ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

mutter_log = open(OUT / "mutter.log", "w")
mutter = subprocess.Popen(
    ["mutter", "--headless", "--wayland", "--no-x11", "--wayland-display", "lull-mutter", "--virtual-monitor", "1920x1080@50"],
    env={**os.environ, "LIBGL_ALWAYS_SOFTWARE": "1", "GSETTINGS_BACKEND": "memory", "GIO_USE_VFS": "local",
         "__EGL_VENDOR_LIBRARY_FILENAMES": "/usr/share/glvnd/egl_vendor.d/50_mesa.json"},
    stdout=mutter_log, stderr=mutter_log)
deadline = time.monotonic() + 15
while not pathlib.Path("/sandbox/run/lull-mutter").exists() and time.monotonic() < deadline:
    time.sleep(0.2)

for test in (opens, playing, crash, closing, sound_killed, no_display, pictures, frame_rate, flashing, in_step_with_sound):
    try:
        test()
    except Exception as error:
        check(test.__name__, False, f"{type(error).__name__}: {error}")
        subprocess.run(["pkill", "-9", "-x", "lull"])
        time.sleep(1)

print(f"{sum(results)}/{len(results)} passed")
sys.exit(0 if all(results) else 1)
