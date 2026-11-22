"""Stress test. The rule: always under 50 MB. Attacks Lull on private servers only.

Samples both processes every 100 ms and fails if their total ever reaches 50 MB of PSS.
Real clicks and keys go through XTEST into a private Xvfb. Run it with scripts/stress.sh [SOAK_SECONDS].
"""
import collections
import ctypes
import json
import os
import pathlib
import random
import re
import signal
import struct
import subprocess
import sys
import threading
import time

if not os.path.isdir("/sandbox/run") or os.path.exists("/dev/snd") or os.path.exists("/dev/dri"):
    sys.exit("refusing to run outside the sandboxes")

LIMIT_MB = 50
LULL = "/lull/target/release/lull"
OUT = pathlib.Path("/out")
WORK = pathlib.Path("/tmp/stress")
FIXTURE = pathlib.Path("/lull/tests/fixtures/noise.opus")
RAIN = next(pathlib.Path("/sandbox/home/Music").glob("*.opus"))
SOAK = int(sys.argv[1]) if len(sys.argv) > 1 else 120
ONLY = sys.argv[2].split(",") if len(sys.argv) > 2 else None
BASE = {key: value for key, value in os.environ.items() if key != "WAYLAND_DISPLAY"}
BASE.update(DISPLAY=":99", __EGL_VENDOR_LIBRARY_FILENAMES="/usr/share/glvnd/egl_vendor.d/50_mesa.json")
random.seed(5)
results = []
WORK.mkdir()

subprocess.Popen(["Xvfb", ":99", "-screen", "0", "3000x2400x24", "-nolisten", "tcp", "-noreset", "-extension", "GLX"],
                 env=BASE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1.5)

xlib = ctypes.CDLL("libX11.so.6")
xtst = ctypes.CDLL("libXtst.so.6")
xlib.XOpenDisplay.restype = ctypes.c_void_p
xlib.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
xlib.XFlush.argtypes = [ctypes.c_void_p]
xlib.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
display = xlib.XOpenDisplay(b":99")
xlib.XSetInputFocus(display, 1, 0, 0)  # Keys go to whatever is under the pointer.
KEYS = {"space": 0x20, "left": 0xFF51, "up": 0xFF52, "right": 0xFF53, "down": 0xFF54, "tab": 0xFF09, "escape": 0xFF1B,
        "return": 0xFF0D}


def move(x, y):
    xtst.XTestFakeMotionEvent(display, 0, int(x), int(y), 0)
    xlib.XFlush(display)


def button(down):
    xtst.XTestFakeButtonEvent(display, 1, int(down), 0)
    xlib.XFlush(display)


def click(x, y, pause=0.02):
    move(x, y)
    button(True)
    button(False)
    time.sleep(pause)


def key(name, pause=0.01):
    code = xlib.XKeysymToKeycode(display, KEYS[name])
    xtst.XTestFakeKeyEvent(display, code, 1, 0)
    xtst.XTestFakeKeyEvent(display, code, 0, 0)
    xlib.XFlush(display)
    time.sleep(pause)


def memory(pid):
    """(PSS, RSS, private) in MB."""
    try:
        text = pathlib.Path(f"/proc/{pid}/smaps_rollup").read_text()
        size = lambda name: int(re.search(rf"^{name}:\s+(\d+)", text, re.M)[1]) / 1024
        return size("Pss"), size("Rss"), size("Private_Clean") + size("Private_Dirty")
    except (OSError, TypeError):
        return 0.0, 0.0, 0.0


def children(pid):
    found = []
    try:
        for task in pathlib.Path(f"/proc/{pid}/task").iterdir():
            found += [int(child) for child in (task / "children").read_text().split()]
    except OSError:
        pass
    return found


class Run:
    """One Lull, watched for its whole life."""

    def __init__(self, name, *args, music=None, env=None):
        self.name = name
        home = WORK / f"home-{name}"
        (home / "state").mkdir(parents=True, exist_ok=True)
        if music:
            (home / "Music").symlink_to(music)
        else:
            (home / "Music").mkdir()
            # A copy: a link out of the music folder is refused, and then nothing would play.
            (home / "Music" / "Rain on Window with Thunder (8h).opus").write_bytes(FIXTURE.read_bytes())
        self.env = {**BASE, "HOME": str(home), "XDG_STATE_HOME": str(home / "state"), **(env or {})}
        self.log = OUT / f"stress-{name}.log"
        self.peak = [0.0, 0.0, 0.0]
        self.samples = []
        self.done = False
        self.home = home
        self.proc = subprocess.Popen([LULL, *args], env=self.env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                     stderr=open(self.log, "w"))
        self.watcher = threading.Thread(target=self.watch)
        self.watcher.start()

    def watch(self):
        while not self.done:
            pids = [self.proc.pid, *children(self.proc.pid)]
            total = [sum(values) for values in zip(*(memory(pid) for pid in pids))]
            self.peak = [max(old, new) for old, new in zip(self.peak, total)]
            self.samples.append(total[0])
            time.sleep(0.1)

    def wait_for(self, text, seconds=12):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and text not in self.log.read_text():
            time.sleep(0.05)
        return text in self.log.read_text()

    def did(self):
        """Counts of logged events: the proof that an attack really landed."""
        counts = collections.Counter()
        for path in (self.home / "state/lull/logs").glob("*.log"):
            counts.update(json.loads(line)["event"] for line in path.read_text().splitlines())
        return counts

    def open_files(self):
        try:
            return len(os.listdir(f"/proc/{self.proc.pid}/fd"))
        except OSError:
            return 0

    def threads(self):
        try:
            return len(os.listdir(f"/proc/{self.proc.pid}/task"))
        except OSError:
            return 0

    def finish(self, expect=(0,), clean=True, note="", need=None, playing=True):
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
        try:
            code = self.proc.wait(15)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            code = "hung"
        self.done = True
        self.watcher.join()
        text = self.log.read_text()
        bad = [word for word in ("process.panic", "audio.underrun", "shutdown.forced") if word in text] if clean else []
        did = self.did()
        if clean and playing:
            need = {"audio.connected": 1, **(need or {})}
        missed = [f"{event} {did[event]}<{count}" for event, count in (need or {}).items() if did[event] < count]
        if missed:
            bad.append("attack didn't land: " + ", ".join(missed))
        ok = self.peak[0] < LIMIT_MB and code in expect and not bad
        results.append((self.name, ok, *self.peak))
        print(f"{'PASS' if ok else 'FAIL'}  {self.name:<24} peak PSS {self.peak[0]:6.1f}  private {self.peak[2]:6.1f}  "
              f"RSS sum {self.peak[1]:6.1f} MB  exit {code} {' '.join(bad)} {note}", flush=True)


def ogg_crc_table():
    table = []
    for value in range(256):
        value <<= 24
        for _ in range(8):
            value = (value << 1) ^ (0x04C11DB7 if value & 0x80000000 else 0)
        table.append(value & 0xFFFFFFFF)
    return table


CRC = ogg_crc_table()


def seal(page):
    page[22:26] = b"\0\0\0\0"
    crc, table = 0, CRC
    for byte in page:
        crc = ((crc << 8) & 0xFFFFFFFF) ^ table[(crc >> 24) ^ byte]
    page[22:26] = struct.pack("<I", crc)
    return bytes(page)


def pages(data):
    at = 0
    while data[at : at + 4] == b"OggS":
        count = data[at + 26]
        size = 27 + count + sum(data[at + 27 : at + 27 + count])
        yield bytearray(data[at : at + size])
        at += size


def with_big_tags(path, megabytes):
    """A valid Opus file whose comment header is huge, as embedded cover art makes it."""
    original = list(pages(FIXTURE.read_bytes()))
    comment = b"METADATA_BLOCK_PICTURE=" + b"A" * (megabytes * 1024 * 1024)
    packet = b"OpusTags" + struct.pack("<I", 4) + b"lull" + struct.pack("<II", 1, len(comment)) + comment
    serial = original[0][14:18]
    lacing = [255] * (len(packet) // 255) + [len(packet) % 255]
    with open(path, "wb") as out:
        out.write(original[0])
        at, sequence = 0, 1
        for first in range(0, len(lacing), 255):
            part = lacing[first : first + 255]
            last = first + 255 >= len(lacing)
            header = bytearray(b"OggS\0" + bytes([1 if first else 0]))
            header += struct.pack("<q", 0 if last else -1) + serial + struct.pack("<I", sequence) + b"\0\0\0\0"
            header += bytes([len(part)]) + bytes(part) + packet[at : at + sum(part)]
            out.write(seal(header))
            at += sum(part)
            sequence += 1
        for page in original[2:]:
            page[18:22] = struct.pack("<I", sequence)
            out.write(seal(page))
            sequence += 1


def with_channels(path):
    """255 channels: a multistream decoder for each would cost tens of MB."""
    original = list(pages(FIXTURE.read_bytes()))
    head = original[0]
    start = 27 + head[26]
    body = bytes(head[start:start + 9]) + bytes([255]) + bytes(head[start + 10:start + 18]) + bytes([255, 255, 0]) + bytes(range(255))
    page = bytearray(head[:26]) + bytes([2, 255, len(body) - 255]) + body
    with open(path, "wb") as out:
        out.write(seal(page))
        for page in original[1:]:
            out.write(seal(page))


def with_links(path, links):
    """A chained file: many short Opus streams end to end, each with its own headers."""
    short = WORK / "short.opus"
    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i", "anoisesrc=r=48000:a=0.05:d=0.1",
                    "-ac", "2", "-c:a", "libopus", "-b:a", "6k", str(short)], check=True, timeout=20)
    template = list(pages(short.read_bytes()))
    with open(path, "wb") as out:
        for link in range(links):
            for page in template:
                page[14:18] = struct.pack("<I", 1000 + link)
                out.write(seal(page))


def soak():
    run = Run("soak-real-file", "--play", "--at", "47:20", str(RAIN))
    run.wait_for("window.opened")
    time.sleep(SOAK)
    settled = run.samples[len(run.samples) // 5 :]
    third = len(settled) // 3
    growth = sum(settled[-third:]) / third - sum(settled[:third]) / third if third else 0
    run.finish(note=f"growth {growth:+.2f} MB over {SOAK} s")


def input_storm():
    run = Run("input-storm", "--play")
    run.wait_for("window.opened")
    time.sleep(1.5)
    fds, threads = run.open_files(), run.threads()
    end = time.monotonic() + 60
    while time.monotonic() < end:
        choice = random.random()
        if choice < 0.2:
            click(56, 480)
        elif choice < 0.35:
            click(random.randint(24, 336), 393)
        elif choice < 0.5:
            move(112, 494)
            button(True)
            for x in range(112, 336, 6):
                move(x, 494)
                time.sleep(0.001)
            button(False)
        elif choice < 0.6:
            click(300, 327)
            key("escape")
        else:
            move(180, 160)
            key(random.choice(["space", "left", "right", "up", "down", "tab", "escape"]), 0.003)
    time.sleep(3)
    run.finish(note=f"fds {fds}->{run.open_files()} threads {threads}->{run.threads()}",
               need={"user.play": 20, "user.pause": 20, "user.seek": 1, "user.volume": 1})


def huge_folder():
    music = WORK / "huge-folder"
    music.mkdir()
    (music / "real.opus").write_bytes(FIXTURE.read_bytes())
    for index in range(5000):
        (music / f"{index:05d} {'Very long focus sound name ' * 8}.opus").symlink_to(music / "real.opus")
    run = Run("5000-sounds", "--play", music=music)
    run.wait_for("window.opened")
    time.sleep(1.5)
    for _ in range(12):
        click(300, 327, 2.0)
        key("down", 0.05)
        key("escape", 0.3)
    run.finish()


def sound_switches():
    music = WORK / "switches"
    music.mkdir()
    for name in ("A", "B", "C"):
        (music / f"{name}.opus").write_bytes(FIXTURE.read_bytes())
    run = Run("60-sound-switches", "--play", music=music)
    run.wait_for("window.opened")
    time.sleep(1.5)
    fds = run.open_files()
    for index in range(60):
        click(300, 327, 0.25)
        click(120, [184, 236, 132][index % 3], 0.45)
    time.sleep(2)
    run.finish(note=f"fds {fds}->{run.open_files()}", need={"user.open": 30, "track.switched": 30})  # The log keeps 30 a minute of one event (src/log/limit.rs).


def many_scripts():
    """Names that make the window load every fallback font the system has."""
    music = WORK / "scripts"
    music.mkdir()
    samples = ["雨の音 集中", "빗소리 집중", "雨声 专注", "صوت المطر", "גשם", "बारिश की आवाज़", "বৃষ্টি", "மழை", "వర్షం", "ಮಳೆ", "മഴ",
               "વરસાદ", "ਮੀਂਹ", "ବର୍ଷା", "ฝน", "ຝົນ", "ភ្លៀង", "မိုး", "ዝናብ", "წვიმა", "անձրև", "ᚱᚨᛁᚾ", "ཆར་པ", "වැස්ස",
               "🌧️⛈️🌊🔥", "𝕽𝖆𝖎𝖓", "Дождь", "Βροχή"]
    for index, sample in enumerate(samples):
        (music / f"{index:02d} {sample} with {sample}.opus").write_bytes(FIXTURE.read_bytes())
    run = Run("every-writing-system", music=music)
    run.wait_for("window.opened")
    time.sleep(2)
    click(300, 327, 3)
    for _ in range(len(samples)):
        key("down", 0.4)
    time.sleep(2)
    run.finish(playing=False)


def enormous_folder():
    music = WORK / "enormous"
    music.mkdir()
    (music / "real.opus").write_bytes(FIXTURE.read_bytes())
    padding = "long name " * 18
    for index in range(150_000):
        os.symlink("real.opus", music / f"{index:06d} {padding}.opus")
    run = Run("150000-sounds", music=music)
    run.wait_for("app.ready", 40)
    time.sleep(3)
    click(300, 327, 8)
    run.finish(playing=False)


def huge_state():
    run_home = WORK / "home-huge-state"
    (run_home / "prepared").mkdir(parents=True)
    state = run_home / "prepared" / "lull"
    state.mkdir()
    for name in ("state", "running"):
        with open(state / name, "wb") as out:
            out.truncate(1 << 30)
    run = Run("1GB-state-files", env={"XDG_STATE_HOME": str(run_home / "prepared")})
    time.sleep(6)
    run.finish(playing=False)


def tiny_loop():
    tiny = WORK / "tiny.opus"
    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i", "anoisesrc=r=48000:a=0.05:d=1.2",
                    "-ac", "2", "-c:a", "libopus", str(tiny)], check=True, timeout=20)
    run = Run("looping-every-second", "--play", str(tiny))
    time.sleep(45)
    run.finish()


def crash_storm():
    run = Run("window-crash-storm", "--play")
    run.wait_for("window.opened")
    for _ in range(5):
        for child in children(run.proc.pid):
            os.kill(child, signal.SIGKILL)
        time.sleep(2)
    run.finish()


def frozen_window():
    run = Run("frozen-window", "--play")
    run.wait_for("window.opened")
    time.sleep(1)
    for child in children(run.proc.pid):
        os.kill(child, signal.SIGSTOP)
    time.sleep(30)
    run.finish()


def server_storm():
    run = Run("sound-server-restarts", "--play")
    run.wait_for("audio.connected")
    for index in range(6):
        subprocess.run(["pkill", "-x", "pipewire-pulse"])
        time.sleep(1.5)
        log = open(OUT / f"stress-pulse-{index}.log", "w")
        subprocess.Popen(["pipewire-pulse"], stdout=log, stderr=log)
        time.sleep(4)
    run.finish(clean=False)


def scales():
    for scale in ("1.25", "1.5", "2", "3", "4", "8"):
        run = Run(f"display-scale-{scale}", "--play", env={"SLINT_SCALE_FACTOR": scale})
        run.wait_for("window.opened")
        time.sleep(4)
        factor = min(float(scale), 3.0)  # Lull caps the scale it passes on.
        click(300 * factor, 327 * factor, 2)
        click(56 * factor, 480 * factor, 2)
        click(56 * factor, 480 * factor, 4)
        run.finish()


def hostile_files():
    cases = []
    for megabytes in (1, 8, 30, 120):
        path = WORK / f"tags-{megabytes}.opus"
        with_big_tags(path, megabytes)
        cases.append((f"cover-art-{megabytes}MB", path))
    for links in (500, 5000, 50000):
        path = WORK / f"links-{links}.opus"
        with_links(path, links)
        cases.append((f"chained-{links}-links", path))
    wide = WORK / "channels.opus"
    with_channels(wide)
    cases.append(("255-channels", wide))
    garbage = WORK / "garbage.opus"
    garbage.write_bytes(os.urandom(5_000_000))
    cases.append(("random-bytes", garbage))
    sparse = WORK / "sparse.opus"
    with open(sparse, "wb") as out:
        out.write(FIXTURE.read_bytes())
        out.truncate(3 << 30)
    cases.append(("3GB-sparse", sparse))
    for name, path in cases:
        run = Run(name, "--play", str(path))
        time.sleep(7)
        run.finish(expect=(0, 1), clean=False)
    music = WORK / "hostile-folder"
    music.mkdir()
    for name, path in cases:
        os.link(path, music / f"{name}.opus")
    (music / "0 good.opus").write_bytes(FIXTURE.read_bytes())
    run = Run("hostile-sounds-list", "--play", music=music)
    run.wait_for("window.opened")
    time.sleep(1.5)
    click(300, 327, 12)
    run.finish(expect=(0, 1), clean=False)


def keeps_playing_without_a_window():
    run = Run("no-display-while-playing", "--play", env={"DISPLAY": ":77"})
    time.sleep(8)
    alive = run.proc.poll() is None
    run.finish(expect=(0,) if alive else (), need={"window.gave_up": 1, "audio.connected": 1})


def stuck_terminal():
    """A terminal paused with Ctrl+S: nobody reads stderr."""
    home = WORK / "home-stuck-terminal"
    (home / "state").mkdir(parents=True)
    (home / "Music").mkdir()
    (home / "Music" / "rain.opus").write_bytes(FIXTURE.read_bytes())
    read_end, write_end = os.pipe()
    os.set_blocking(write_end, False)
    try:
        while True:
            os.write(write_end, b"x" * 4096)
    except BlockingIOError:
        os.set_blocking(write_end, True)
    proc = subprocess.Popen([LULL, "--no-window", "--play"], env={**BASE, "HOME": str(home), "XDG_STATE_HOME": str(home / "state")},
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=write_end)
    time.sleep(3)
    proc.send_signal(signal.SIGTERM)
    try:
        code = proc.wait(6)
    except subprocess.TimeoutExpired:
        proc.kill()
        code = "hung"
    os.close(read_end)
    os.close(write_end)
    ok = code == 0 and (home / "state/lull/state").exists()
    results.append(("stuck-terminal", ok, 0.0, 0.0, 0.0))
    print(f"{'PASS' if ok else 'FAIL'}  stuck-terminal           exit {code}, saved and quit with stderr blocked", flush=True)


TESTS = (soak, input_storm, huge_folder, sound_switches, tiny_loop, crash_storm, frozen_window, server_storm, scales,
         hostile_files, many_scripts, enormous_folder, huge_state, keeps_playing_without_a_window, stuck_terminal)
for test in TESTS:
    if ONLY and test.__name__ not in ONLY:
        continue
    try:
        test()
    except Exception as error:
        results.append((test.__name__, False, 0, 0, 0))
        print(f"FAIL  {test.__name__}  {type(error).__name__}: {error}", flush=True)
        subprocess.run(["pkill", "-9", "-x", "lull"])
        time.sleep(1)

worst = max(results, key=lambda row: row[2])
print(f"{sum(row[1] for row in results)}/{len(results)} passed. Worst peak: {worst[2]:.1f} MB PSS ({worst[0]})")
sys.exit(0 if all(row[1] for row in results) else 1)
