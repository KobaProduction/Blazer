"""Real Android VPN/JNI leastPing acceptance on the isolated Blazer_API_35 AVD.

Requires a local debug APK built with build-android.ps1 -Acceptance and a trusted
selector-capable BlazeCore bundle. Installs only on the named emulator, imports
into an isolated catalog, and forwards loopback fixture ports through adb reverse.
"""

import argparse
import base64
import importlib.util
import json
import os
from pathlib import Path
import queue
import re
import subprocess
import threading
import time


NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)
PACKAGE = "com.kobaproduction.blazer"
spec = importlib.util.spec_from_file_location(
    "blazer_native_fixtures", Path(__file__).with_name("probe-balancer-contract.py")
)
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)


def document(a, b, c):
    def outbound(tag, server):
        return {"tag": tag, "protocol": "socks", "settings": {
            "servers": [{"address": "127.0.0.1", "port": server.server_address[1]}]
        }}
    return {
        "remarks": "Android native leastPing acceptance",
        "outbounds": [outbound("global", c), outbound("candidate-a", a), outbound("candidate-b", b)],
        "routing": {"domainStrategy": "AsIs", "balancers": [{
            "tag": "least", "selector": ["candidate-"], "strategy": {"type": "leastPing"},
            "fallbackTag": "global",
        }], "rules": [{"type": "field", "network": "tcp", "balancerTag": "least"}]},
        "observatory": {"subjectSelector": ["candidate-"],
            "probeURL": "http://health.fixture.invalid/check", "probeInterval": "200ms",
            "enableConcurrency": True},
    }


def adb_tool(options, *arguments, timeout=30, check=True):
    result = subprocess.run(
        [str(options.adb), "-s", options.serial, *arguments], capture_output=True,
        text=True, creationflags=NO_WINDOW, timeout=timeout,
    )
    if check:
        assert result.returncode == 0, f"adb {arguments[0]} failed: {result.stdout} {result.stderr}"
    return result.stdout.strip()


def require_isolated_avd(options):
    assert options.serial.startswith("emulator-"), "physical Android devices are excluded"
    assert adb_tool(options, "shell", "getprop", "ro.kernel.qemu") == "1", "target is not an emulator"
    name = adb_tool(options, "emu", "avd", "name").splitlines()[0]
    assert name == "Blazer_API_35", f"unexpected AVD: {name}"
    deadline = time.monotonic() + 120
    while adb_tool(options, "shell", "getprop", "sys.boot_completed") != "1":
        assert time.monotonic() < deadline, "AVD boot timed out"
        time.sleep(1)


def instrument(options, configuration, a, b):
    encoded = base64.b64encode(json.dumps(configuration).encode()).decode()
    process = subprocess.Popen(
        [str(options.adb), "-s", options.serial, "shell", "am", "instrument", "-w", "-r",
         "-e", "config", encoded, PACKAGE + "/.AcceptanceInstrumentation"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        creationflags=NO_WINDOW,
    )
    lines = queue.Queue()

    def drain():
        for line in process.stdout:
            lines.put(line.rstrip())
        lines.put(None)

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    observed = []
    deadline = time.monotonic() + 150
    try:
        while time.monotonic() < deadline:
            try:
                line = lines.get(timeout=0.5)
            except queue.Empty:
                continue
            if line is None:
                break
            observed.append(line)
            print(line, flush=True)
            if "stage=fail_a" in line:
                a.healthy = False
            elif "stage=all_dead" in line:
                b.healthy = False
            elif "stage=recover_a" in line:
                a.healthy = True
        else:
            raise AssertionError("Android native acceptance timed out")
        assert process.wait(timeout=5) == 0, "adb instrumentation process failed"
        output = "\n".join(observed)
        assert "PASS leastPing A->B->fallback C->A" in output, output
        assert "INSTRUMENTATION_CODE: 0" in output, output
    finally:
        if process.poll() is None:
            process.terminate()
        process.wait(timeout=5)
        reader.join(timeout=2)
        process.stdout.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adb", type=Path, default=Path(os.environ.get("ANDROID_HOME", "")) / "platform-tools/adb.exe")
    parser.add_argument("--serial", default="emulator-5556")
    parser.add_argument("--apk", type=Path, default=Path("target/android/acceptance/blazer-acceptance.apk"))
    options = parser.parse_args()
    options.adb = options.adb.resolve(strict=True)
    apk = options.apk.resolve(strict=True)
    require_isolated_avd(options)
    adb_tool(options, "install", "-r", str(apk), timeout=90)
    previous = adb_tool(options, "shell", "cmd", "appops", "get", PACKAGE, "ACTIVATE_VPN", check=False)
    mode = re.search(r"ACTIVATE_VPN:\s+(allow|ignore|deny|default)", previous)
    forwarded = []
    try:
        adb_tool(options, "shell", "cmd", "appops", "set", PACKAGE, "ACTIVATE_VPN", "allow")
        adb_tool(options, "shell", "am", "force-stop", PACKAGE)
        with fixtures.upstream("A", 0.005) as a, fixtures.upstream("B", 0.150) as b, fixtures.upstream("C", 0) as c:
            for server in (a, b, c):
                server.socket_timeout = 60
                server.allowed_destinations = {("health.fixture.invalid", 80), ("198.51.100.213", 443)}
                port = server.server_address[1]
                adb_tool(options, "reverse", f"tcp:{port}", f"tcp:{port}")
                forwarded.append(port)
            instrument(options, document(a, b, c), a, b)
            assert not a.errors and not b.errors and not c.errors
            assert a.probes and b.probes, "native shared observer never reached both subjects"
            assert set(a.probe_methods + b.probe_methods) == {"GET"}
    finally:
        adb_tool(options, "shell", "am", "force-stop", PACKAGE, check=False)
        for port in forwarded:
            adb_tool(options, "reverse", "--remove", f"tcp:{port}", check=False)
        adb_tool(options, "shell", "cmd", "appops", "set", PACKAGE, "ACTIVATE_VPN", mode[1] if mode else "default", check=False)


if __name__ == "__main__":
    main()
