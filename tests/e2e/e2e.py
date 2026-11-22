"""End-to-end checks of the real binary on the private sound server from scripts/sandbox.sh.

Records the virtual speaker and measures it. Standard library only. Run it with scripts/e2e.sh.
"""
import array
import json
import os
import pathlib
import re
import signal
import subprocess
import sys
import time

if not os.path.isdir("/sandbox/run") or os.path.exists("/dev/snd"):
    sys.exit("refusing to run outside scripts/sandbox.sh")

LULL = "/lull/target/release/lull"
OUT = pathlib.Path("/out")
STATE = pathlib.Path("/sandbox/state/lull/state")
RATE = 48000
results = []


def check(name, ok, detail=""):
    results.append(ok)
    print(f"{'PASS' if ok else 'FAIL'}  {name}  {detail}", flush=True)


def sh(*args):
    return subprocess.run(args, capture_output=True, text=True, timeout=10).stdout


def fresh(volume=100):
    STATE.parent.mkdir(parents=True, exist_ok=True)
    STATE.write_bytes(f"lull-state 1\nposition=0\nvolume={volume}\ntrack=".encode())


def saved_position():
    return int(re.search(rb"position=(\d+)", STATE.read_bytes())[1])


class Lull:
    count = 0

    def __init__(self, *args):
        Lull.count += 1
        self.log = OUT / f"lull-{Lull.count}.log"
        self.file = open(self.log, "w")
        self.proc = subprocess.Popen([LULL, "--no-window", *args], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=self.file)

    def press(self, keys):
        self.proc.stdin.write(keys)
        self.proc.stdin.flush()

    def text(self):
        return self.log.read_text()

    def wait_for(self, text, seconds=12.0):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if text in self.text():
                return True
            time.sleep(0.02)
        return False

    def stop(self):
        started = time.monotonic()
        self.proc.send_signal(signal.SIGTERM)
        try:
            code = self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            code = -9
        self.file.close()
        return code, time.monotonic() - started

    def ticks(self):
        fields = pathlib.Path(f"/proc/{self.proc.pid}/stat").read_text().rsplit(")", 1)[1].split()
        return int(fields[11]) + int(fields[12])

    def cpu_percent(self, seconds):
        before = self.ticks()
        time.sleep(seconds)
        return (self.ticks() - before) / os.sysconf("SC_CLK_TCK") / seconds * 100

    def pss_mb(self):
        text = pathlib.Path(f"/proc/{self.proc.pid}/smaps_rollup").read_text()
        return int(re.search(r"^Pss:\s+(\d+)", text, re.M)[1]) / 1024


class Recorder:
    def __init__(self, name, sink="test_a"):
        self.path = OUT / f"{name}.raw"
        self.file = open(self.path, "wb")
        self.proc = subprocess.Popen(
            ["parec", f"--device={sink}.monitor", "--format=float32le", "--rate=48000", "--channels=2",
             "--latency-msec=20", "--raw"], stdout=self.file)
        time.sleep(0.5)

    def stop(self):
        time.sleep(0.4)
        self.proc.terminate()
        self.proc.wait()
        self.file.close()
        data = self.path.read_bytes()
        return data[: len(data) // 8 * 8]


def left(data):
    samples = array.array("f")
    samples.frombytes(data)
    return samples[0::2]


def audible(data):
    """(first, last) audible frame, or None."""
    channel = left(data)
    hits = [i for i in range(0, len(channel), 8) if abs(channel[i]) > 1e-6]
    return (hits[0], hits[-1]) if hits else None


def longest_gap_ms(data):
    found = audible(data)
    if not found:
        return float("inf")
    longest = run = 0
    for sample in left(data)[found[0] + 2400:max(found[0] + 2400, found[1] - 2400)]:
        run = run + 1 if abs(sample) < 1e-9 else 0
        longest = max(longest, run)
    return longest / 48


def node_ids():
    """Sink name to PipeWire node id. pactl's index is a different number, which pw-cli refuses."""
    found = {}
    for node in json.loads(sh("pw-dump")):
        props = (node.get("info") or {}).get("props") or {}
        if props.get("media.class") == "Audio/Sink":
            found[props["node.name"]] = str(node["id"])
    return found


def sink_names():
    return dict(line.split("\t")[:2] for line in sh("pactl", "list", "short", "sinks").splitlines() if line)


def streams():
    return [line.split("\t") for line in sh("pactl", "list", "short", "sink-inputs").splitlines() if line]


def sample_accurate():
    fresh()
    recorder = Recorder("exact")
    lull = Lull("--at", "47:20", "--play")
    lull.wait_for("audio.connected")
    time.sleep(6)
    code, took = lull.stop()
    data = recorder.stop()
    start = audible(data)[0] + RATE
    reference = (OUT / "reference.raw").read_bytes()
    probe = data[start * 8 : start * 8 + 512]
    at = reference.find(probe)
    while at >= 0 and at % 8:
        at = reference.find(probe, at + 1)
    size = 3 * RATE * 8
    same = at >= 0 and data[start * 8 : start * 8 + size] == reference[at : at + size]
    check("at 100% the speaker gets the decoded file bit for bit", same, f"3 s compared, found at frame {at // 8}")
    check("quit exits cleanly and quickly", code == 0 and took < 2.0, f"code {code}, {took:.2f} s")
    first, last = audible(data)
    played = (last - first) / RATE
    moved = (saved_position() - 2840 * RATE) / RATE
    check("saved position equals what was heard", abs(moved - played) < 0.1, f"saved {moved:.3f} s, heard {played:.3f} s")
    tail = [abs(s) for s in left(data)[last - 14400 : last]]
    quarters = [sum(tail[i : i + 3600]) / 3600 for i in range(0, 14400, 3600)]
    check("the 300 ms fade out is heard in full", quarters == sorted(quarters, reverse=True) and quarters[3] < quarters[0] / 4,
          " > ".join(f"{q:.4f}" for q in quarters))


def controls():
    fresh(20)
    recorder = Recorder("controls")
    lull = Lull("--at", "47:20")
    lull.wait_for("app.ready")
    time.sleep(1)
    check("starts paused, with no stream open", not streams())
    lull.press(b" ")
    time.sleep(3)
    check("play opens one stream", len(streams()) == 1)
    lull.press(b"\x1b[C")
    time.sleep(1)
    lull.press(b"\x1b[A")
    time.sleep(1)
    pressed = time.monotonic()
    lull.press(b" ")
    closed = lull.wait_for("audio.paused", 4)
    took = time.monotonic() - pressed
    time.sleep(0.3)
    check("pause fades, then releases the device", closed and not streams(), f"stream closed {took:.2f} s after the key")
    paused_at = saved_position() / RATE
    check("seek moved 30 s", abs(paused_at - (2840 + 5.3 + 30)) < 0.8, f"at {paused_at:.2f} s")
    lull.press(b" ")
    time.sleep(2)
    pauses = lull.text().count("audio.paused")
    for _ in range(5):
        lull.press(b" ")
        time.sleep(0.12)
    time.sleep(2)
    check("rapid presses settle on the last one", lull.text().count("audio.paused") == pauses + 1 and not streams())
    code, took = lull.stop()
    check("quit while paused is instant", code == 0 and took < 0.4, f"{took:.2f} s")
    recorder.stop()
    check("no underrun in normal use", "audio.underrun" not in lull.text())


def loop_join():
    fresh()
    recorder = Recorder("loop")
    lull = Lull("--at", "8:00:05", "--play")
    lull.wait_for("audio.connected")
    time.sleep(12)
    code, _ = lull.stop()
    data = recorder.stop()
    position = saved_position() / RATE
    check("the real 8 h file loops to the start", code == 0 and 5 <= position < 20, f"ended at {position:.2f} s")
    gap = longest_gap_ms(data)
    check("no gap at the loop join", gap < 1, f"longest silent run {gap:.2f} ms")
    peak = max(abs(s) for s in left(data))
    check("the loop join never clips", peak <= 1.0, f"peak {peak:.3f}")


def default_device_change():
    fresh()
    recorder = Recorder("moved", "test_b")
    lull = Lull("--at", "47:20", "--play")
    lull.wait_for("audio.connected")
    time.sleep(2)
    sh("pactl", "set-default-sink", "test_b")
    time.sleep(3)
    sinks = sink_names()
    now_on = [sinks.get(stream[1]) for stream in streams()]
    sh("pactl", "set-default-sink", "test_a")
    time.sleep(2)
    lull.stop()
    check("follows a change of default device", now_on == ["test_b"] and audible(recorder.stop()) is not None, str(now_on))


def cpu_contention():
    fresh(20)
    cpu = str(min(os.sched_getaffinity(0)))
    lull = Lull("--at", "47:20", "--play")
    lull.wait_for("audio.connected")
    sh("taskset", "-a", "-p", "-c", cpu, str(lull.proc.pid))
    hog = subprocess.Popen(["taskset", "-c", cpu, sys.executable, "-c", "while True: pass"])
    time.sleep(20)
    hog.kill()
    hog.wait()
    lull.stop()
    check("no underrun sharing one saturated CPU core", "audio.underrun" not in lull.text(), "20 s")


def cost():
    fresh(20)
    lull = Lull("--at", "47:20")
    lull.wait_for("app.ready")
    time.sleep(1)
    cpu = lull.cpu_percent(10)
    pss = lull.pss_mb()
    check("paused: under 15 MB and 0.5% CPU", pss < 15 and cpu < 0.5, f"{pss:.1f} MB PSS, {cpu:.2f}% CPU")
    lull.press(b" ")
    time.sleep(5)
    cpu = lull.cpu_percent(40)
    pss = lull.pss_mb()
    check("playing: under 15 MB and 3% CPU", pss < 15 and cpu < 3, f"{pss:.1f} MB PSS, {cpu:.2f}% CPU")
    lull.stop()
    check("no underrun while measuring", "audio.underrun" not in lull.text())


def server_restart():
    fresh()
    lull = Lull("--at", "47:20", "--play")
    lull.wait_for("audio.connected")
    time.sleep(4)
    subprocess.run(["pkill", "-x", "pipewire-pulse"])
    noticed = lull.wait_for("audio.lost", 8)
    time.sleep(3)
    log = open(OUT / "pipewire-pulse-2.log", "w")
    subprocess.Popen(["pipewire-pulse"], stdout=log, stderr=log)
    back = lull.wait_for("audio.restored", 12)
    check("survives a sound server restart", noticed and back)
    match = re.search(r"audio\.restored at=47:(\d\d)", lull.text())
    check("resumes at the same place", bool(match) and 22 <= int(match[1]) <= 25, match[0] if match else "")
    time.sleep(2)
    check("sound flows after the restart", len(streams()) == 1)
    lull.stop()


def frozen_server():
    fresh()
    lull = Lull("--at", "47:20", "--play")
    lull.wait_for("audio.connected")
    time.sleep(2)
    subprocess.run(["pkill", "-STOP", "-x", "pipewire-pulse"])
    noticed = lull.wait_for("audio.lost", 9)
    code, took = lull.stop()
    subprocess.run(["pkill", "-CONT", "-x", "pipewire-pulse"])
    time.sleep(1)
    check("a frozen sound server is noticed", noticed)
    check("quit works with a frozen server", code == 0 and took < 7, f"code {code}, {took:.2f} s")


def device_unplugged():
    fresh()
    recorder = Recorder("unplug", "test_b")
    lull = Lull("--at", "47:20", "--play")
    lull.wait_for("audio.connected")
    time.sleep(2)
    sh("pw-cli", "destroy", node_ids()["test_a"])
    time.sleep(4)
    gone = "test_a" not in sink_names().values()
    alive = lull.proc.poll() is None and gone
    lull.stop()
    check("moves over when the device is unplugged", alive and audible(recorder.stop()) is not None)


def last_device_gone():
    fresh()
    lull = Lull("--at", "47:20", "--play")
    lull.wait_for("audio.connected")
    time.sleep(1)
    for node in node_ids().values():
        sh("pw-cli", "destroy", node)
    time.sleep(8)
    print("      sinks now:", sink_names(), "streams:", streams())
    removed = not {"test_a", "test_b"} & set(sink_names().values())
    noticed = "audio.lost" in lull.text()
    alive = lull.proc.poll() is None
    code, took = lull.stop()
    moved = saved_position() / RATE - 2840
    check("with no device at all it says so", noticed and "no sound device is connected" in lull.text())
    check("with no device at all it waits and still quits", removed and alive and code == 0 and took < 7 and moved < 3,
          f"{took:.2f} s, noticed={noticed}, advanced {moved:.1f} s of 9")


def device_returns():
    fresh()
    lull = Lull("--at", "47:20", "--play")
    lull.wait_for("audio.lost")
    time.sleep(20)
    lines = lull.text().count("audio.lost")
    sh("pactl", "load-module", "module-null-sink", "sink_name=test_c")
    back = lull.wait_for("audio.restored", 9)
    time.sleep(2)
    names = [sink_names().get(stream[1]) for stream in streams()]
    lull.stop()
    moved = saved_position() / RATE - 2840
    check("plays again when a device returns", back and names == ["test_c"], str(names))
    check("keeps the place while waiting", 1.0 < moved < 6.0, f"moved {moved:.2f} s across a 20 s+ wait")
    check("waiting does not flood the log", lines <= 6, f"{lines} lines")


# The destructive ones go last.
for test in (sample_accurate, controls, loop_join, default_device_change, cpu_contention, cost,
             server_restart, frozen_server, device_unplugged, last_device_gone, device_returns):
    try:
        test()
    except Exception as error:
        check(test.__name__, False, f"{type(error).__name__}: {error}")

print(f"{sum(results)}/{len(results)} passed")
sys.exit(0 if all(results) else 1)
