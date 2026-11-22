"""Packaged Blazer: HTTP Xray subscription -> persisted graph -> real round-robin.

Uses isolated catalogs and two loopback SOCKS fixtures. No Windows proxy/TUN
changes, user subscriptions, external HTTP requests or BlazeCore source changes.
"""

import argparse
import base64
import contextlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import queue
import re
import socket
import subprocess
import tempfile
import threading
import time


NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)
spec = importlib.util.spec_from_file_location(
    "balancer_contract", Path(__file__).with_name("probe-balancer-contract.py")
)
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)


def cli(executable, environment, *arguments, input_text=""):
    result = subprocess.run(
        [str(executable), "--cli", *arguments], input=input_text,
        capture_output=True, text=True, env=environment,
        creationflags=NO_WINDOW, timeout=40,
    )
    assert result.returncode == 0, f"CLI {arguments}: {result.stdout} {result.stderr}"
    return result.stdout


def environment_for(executable, directory):
    environment = os.environ.copy()
    environment.update(
        BLAZER_DATA_DIR=str(directory), OTEL_SDK_DISABLED="true",
        BLAZER_BLAZECORE_PATH=str(executable.parent / "blazecore.exe"),
    )
    return environment


def xray_document(a, b):
    def outbound(tag, port):
        return {"tag": tag, "protocol": "socks", "settings": {
            "servers": [{"address": "127.0.0.1", "port": port}]
        }}

    return {
        "remarks": "Two-server round-robin",
        "inbounds": [{"tag": "local", "listen": "127.0.0.1", "port": 1080,
                      "protocol": "socks", "settings": {"auth": "noauth"}}],
        "outbounds": [outbound("edge-a", a), outbound("edge-b", b),
                      {"tag": "blocked", "protocol": "blackhole", "settings": {}}],
        "routing": {
            "domainStrategy": "AsIs",
            "balancers": [{"tag": "balanced", "selector": ["edge-"],
                           "strategy": {"type": "roundRobin"}}],
            "rules": [
                {"type": "field", "domain": ["full:blocked.fixture.invalid"], "outboundTag": "blocked"},
                {"type": "field", "domain": ["full:fixed.fixture.invalid"], "outboundTag": "edge-b"},
                {"type": "field", "inboundTag": ["local"], "network": "tcp", "balancerTag": "balanced"},
            ],
        },
    }


class Subscription(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        data = self.server.document
        if self.server.encoded:
            data = base64.b64encode(data)
        self.server.requests += 1
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_):
        pass


@contextlib.contextmanager
def subscription(document):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Subscription)
    server.document = json.dumps(document).encode()
    server.encoded, server.requests = False, 0
    thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
    thread.start()
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


def single_graph(executable, environment):
    output = cli(executable, environment, "profiles")
    rows = re.findall(r"^profile=([0-9a-f-]+) protocol=Xray graph ", output, re.MULTILINE)
    assert len(rows) == 1 and "profile_unavailable=" not in output, output
    assert len(re.findall(r"^profile=", output, re.MULTILINE)) == 1, output
    return rows[0]


@contextlib.contextmanager
def running_proxy(executable, environment):
    process = subprocess.Popen(
        [str(executable), "--cli", "proxy"], env=environment,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, creationflags=NO_WINDOW,
    )
    lines = queue.Queue()
    output = []

    def drain():
        for line in process.stdout:
            output.append(line.rstrip())
            lines.put(line.rstrip())
        lines.put(None)

    thread = threading.Thread(target=drain, daemon=True)
    thread.start()
    try:
        deadline = time.monotonic() + 15
        while True:
            remaining = deadline - time.monotonic()
            assert remaining > 0, f"proxy startup timed out: {output}"
            line = lines.get(timeout=remaining)
            assert line is not None, f"proxy failed to start: {output}"
            if line.startswith("local_proxy="):
                port = int(line.rsplit(":", 1)[1])
                break
        yield port
        process.stdin.write("\n")
        process.stdin.flush()
        assert process.wait(timeout=10) == 0, output
        with socket.socket() as released:
            released.bind(("127.0.0.1", port))
    finally:
        if process.poll() is None:
            process.terminate()
        process.wait(timeout=5)
        thread.join(timeout=2)
        process.stdin.close()
        process.stdout.close()


def check_round_robin(port):
    fixtures.rejected(port, "blocked.fixture.invalid")
    assert fixtures.route(port, "fixed.fixture.invalid") == "B", "explicit outbound route lost"
    observed = [fixtures.route(port, "balanced.fixture.invalid") for _ in range(6)]
    assert observed == ["A", "B", "A", "B", "A", "B"], observed
    with fixtures.connect(port, "balanced.fixture.invalid") as established:
        assert fixtures.exchange(established) == "A"
        assert [fixtures.route(port, "balanced.fixture.invalid") for _ in range(3)] == ["B", "A", "B"]
        assert fixtures.exchange(established) == "A", "established flow changed its selected server"
    print("PASS Blazer round-robin: A B A B A B; existing connection stays on A")


def check_pipeline(executable, directory, a, b):
    document = xray_document(a.server_address[1], b.server_address[1])
    environment = environment_for(executable, directory / "subscription")
    with subscription(document) as server:
        cli(executable, environment, "subscription", "add", "Balanced fixture",
            input_text=f"http://127.0.0.1:{server.server_address[1]}/subscription\n")
        profile = single_graph(executable, environment)
        subscriptions = cli(executable, environment, "subscriptions")
        match = re.search(r"^subscription=([0-9a-f-]+) profiles=1 ", subscriptions, re.MULTILINE)
        assert match, subscriptions
        cli(executable, environment, "profile", "select", profile)
        with running_proxy(executable, environment) as port:
            check_round_robin(port)
        print("PASS HTTP subscription -> protected catalog -> new CLI process -> graph startup/stop")

        server.encoded = True
        cli(executable, environment, "subscription", "refresh", match[1])
        assert single_graph(executable, environment) == profile, "refresh changed graph identity"
        routing = cli(executable, environment, "routing", "add", "Composed policy",
                      input_text="block domain user-blocked.fixture.invalid\nproxy suffix fixture.invalid\n")
        saved = re.search(r"routing_profile_saved=([0-9a-f-]+)", routing)
        assert saved, routing
        cli(executable, environment, "routing", "assign", "global", saved[1], "enabled")
        with running_proxy(executable, environment) as port:
            fixtures.rejected(port, "user-blocked.fixture.invalid")
            check_round_robin(port)
        assert server.requests == 2, server.requests
        print("PASS Base64 refresh preserves identity; user policy composes with provider block/routes/group")

    manual = environment_for(executable, directory / "manual")
    cli(executable, manual, "import", input_text=json.dumps(document))
    single_graph(executable, manual)
    with running_proxy(executable, manual) as port:
        check_round_robin(port)
    assert not a.errors and not b.errors, (a.errors, b.errors)
    print("PASS manual full-Xray import uses the same persisted graph and real traffic")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, default=Path("dist/windows/client/blazer.exe"))
    executable = parser.parse_args().executable.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="blazer-xray-balancer-") as temporary:
        with fixtures.upstream("A", 0) as a, fixtures.upstream("B", 0) as b:
            check_pipeline(executable, Path(temporary), a, b)


if __name__ == "__main__":
    main()
