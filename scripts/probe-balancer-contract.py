"""Bounded native BlazeCore routing/group acceptance using loopback upstreams.

No TUN, OS proxy settings, installation, or source changes. Automatic health
checks name a public host (guarded DNS may resolve it), but fixture SOCKS servers
answer the HTTP checks locally. No user data or external HTTP traffic is used.
"""

import argparse
from collections import Counter
import contextlib
import hashlib
import http.client
import json
import os
from pathlib import Path
import secrets
import socket
import socketserver
import subprocess
import tempfile
import threading
import time


VERSION = "0.3.7+gb90121c0406c"
SHA256 = "CCA53DEC21570B841D2CBC9F414410CA5CB84F1209865296B31DCB89837E5DCF"
NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)


def exact(stream, count):
    result = bytearray()
    while len(result) < count:
        data = stream.recv(count - len(result))
        if not data:
            raise EOFError("fixture connection closed")
        result.extend(data)
    return bytes(result)


def destination(stream):
    version, command, _, kind = exact(stream, 4)
    assert (version, command) == (5, 1)
    if kind == 3:
        host = exact(stream, exact(stream, 1)[0]).decode("ascii")
    elif kind == 1:
        host = socket.inet_ntoa(exact(stream, 4))
    elif kind == 4:
        host = socket.inet_ntop(socket.AF_INET6, exact(stream, 16))
    else:
        raise AssertionError("invalid SOCKS address type")
    return host, int.from_bytes(exact(stream, 2), "big")


class Upstream(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            self.request.settimeout(getattr(self.server, "socket_timeout", 5))
            version, count = exact(self.request, 2)
            assert version == 5
            exact(self.request, count)
            self.request.sendall(b"\x05\x00")
            target = destination(self.request)
            allowed = getattr(self.server, "allowed_destinations", None)
            if allowed is not None and target not in allowed:
                # Isolated Android VPNs can also receive OS connectivity/DNS
                # traffic. Reject it without contacting any external target.
                self.request.sendall(b"\x05\x02\x00\x01\x7f\x00\x00\x01\x00\x00")
                return
            self.request.sendall(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00")
            while True:
                payload = exact(self.request, 4)
                if payload in (b"HEAD", b"GET "):
                    headers = payload
                    while b"\r\n\r\n" not in headers and len(headers) < 16384:
                        headers += exact(self.request, 1)
                    self.server.probes += 1
                    self.server.probe_methods.append(payload.strip().decode("ascii"))
                    self.server.probe_times.append(time.monotonic())
                    if not self.server.healthy:
                        return
                    time.sleep(self.server.delay)
                    status = getattr(self.server, "probe_status", 204)
                    self.request.sendall(f"HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".encode("ascii"))
                    return
                assert payload == b"PING"
                self.server.payloads.append(target)
                self.request.sendall(self.server.label.encode() + b":" + payload)
        except (OSError, EOFError):
            pass
        except AssertionError as error:
            self.server.errors.append(str(error))


class Fixture(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True


@contextlib.contextmanager
def upstream(label, delay):
    server = Fixture(("127.0.0.1", 0), Upstream)
    server.label, server.delay, server.healthy = label, delay, True
    server.payloads, server.errors, server.probes = [], [], 0
    server.probe_methods = []
    server.probe_times = []
    worker = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
    worker.start()
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=2)


def free_port():
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        return reservation.getsockname()[1]


def member(name):
    return {"chain": [name]}


def config(port, controller, secret, a, b, dead):
    def client(name, address):
        return {"client_group": name, "client_chain": {"name": name + "-leaf", "address": address, "protocol": {"type": "socks"}}}

    def rule(conditions, group=None, block=False):
        value = {"conditions": conditions, "action": "block" if block else "allow", "no_resolve": True}
        if group is not None:
            value["client_chain"] = group
        return value

    return [
        client("a", f"127.0.0.1:{a}"), client("b", f"127.0.0.1:{b}"),
        client("dead", f"127.0.0.1:{dead}"),
        {"client_group": "balanced", "type": "round-robin", "client_chains": [member("a"), member("b")]},
        {"client_group": "weighted", "type": "round-robin", "client_chains": [member("a"), member("a"), member("b")]},
        {"client_group": "manual", "type": "selector", "default": "b", "client_chains": [member("a"), member("b")]},
        {"client_group": "automatic", "type": "url-test", "url": "http://connectivitycheck.gstatic.com/generate_204", "interval": 1, "timeout": 750, "tolerance": 0, "lazy": False, "client_chains": [member("a"), member("b")]},
        {"client_group": "failure", "type": "round-robin", "client_chains": [member("dead"), member("b")]},
        {"clash_api_address": f"127.0.0.1:{controller}", "secret": secret},
        {"address": f"127.0.0.1:{port}", "protocol": {"type": "socks"}, "rules": [
            rule({"domain": ["blocked.route.invalid"]}, block=True),
            rule({"domain": ["first.route.invalid"]}, "a"),
            rule({"domain": ["manual.route.invalid"]}, "manual"),
            rule({"domain": ["auto.route.invalid"]}, "automatic"),
            rule({"domain": ["weighted.route.invalid"]}, "weighted"),
            rule({"domain": ["failure.route.invalid"]}, "failure"),
            rule({"domain_suffix": ["route.invalid"]}, "b"),
            rule({"ip_cidr": ["198.51.100.0/24"]}, "a"),
            rule({"any_destination": True}, "balanced"),
        ]},
    ]


def core_check(executable, directory, document):
    path = directory / "check.json"
    path.write_text(json.dumps(document), encoding="utf-8")
    return subprocess.run([str(executable), "check", str(path)], capture_output=True, text=True,
                          creationflags=NO_WINDOW, timeout=10)


@contextlib.contextmanager
def running_core(executable, directory, document):
    path = directory / "runtime.json"
    path.write_text(json.dumps(document), encoding="utf-8")
    process = subprocess.Popen([str(executable), str(path)], stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                               creationflags=NO_WINDOW, cwd=directory)
    ready = threading.Event()
    output = []

    def drain():
        for line in process.stdout:
            if len(output) < 30:
                output.append(line.rstrip())
            if line.strip() == "Servers ready":
                ready.set()

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    try:
        assert ready.wait(10), f"core did not become ready: {output}"
        yield
    finally:
        if process.poll() is None:
            process.terminate()
        process.wait(timeout=5)
        reader.join(timeout=2)
        process.stdout.close()


def connect(port, host):
    stream = socket.create_connection(("127.0.0.1", port), timeout=3)
    try:
        stream.settimeout(3)
        stream.sendall(b"\x05\x01\x00")
        assert exact(stream, 2) == b"\x05\x00"
        try:
            address = b"\x01" + socket.inet_aton(host)
        except OSError:
            encoded = host.encode("ascii")
            address = b"\x03" + bytes([len(encoded)]) + encoded
        stream.sendall(b"\x05\x01\x00" + address + (443).to_bytes(2, "big"))
        reply = exact(stream, 4)
        if reply[1] != 0:
            raise OSError("core rejected the route")
        if reply[3] == 1:
            exact(stream, 6)
        elif reply[3] == 4:
            exact(stream, 18)
        else:
            exact(stream, exact(stream, 1)[0] + 2)
        return stream
    except BaseException:
        stream.close()
        raise


def exchange(stream):
    stream.sendall(b"PING")
    return exact(stream, 6)[:1].decode("ascii")


def route(port, host):
    with connect(port, host) as stream:
        return exchange(stream)


def rejected(port, host):
    try:
        route(port, host)
    except (OSError, EOFError):
        return
    raise AssertionError(f"route unexpectedly succeeded: {host}")


def api(port, secret, name, selected=None):
    with contextlib.closing(http.client.HTTPConnection("127.0.0.1", port, timeout=2)) as connection:
        headers = {"Authorization": "Bearer " + secret}
        body = None if selected is None else json.dumps({"name": selected})
        connection.request("GET" if selected is None else "PUT", f"/proxies/{name}", body, headers)
        reply = connection.getresponse()
        data = reply.read()
        assert reply.status in (200, 204), f"controller returned {reply.status}"
        return json.loads(data) if data else None


def await_selected(controller, secret, selected):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        state = api(controller, secret, "automatic")
        if state.get("now") == selected and state.get("health") == "healthy":
            return
        time.sleep(0.1)
    raise AssertionError(f"automatic group did not select healthy {selected}: {state}")


def check_runtime(port, controller, secret, a, b):
    rejected(port, "blocked.route.invalid")
    assert route(port, "first.route.invalid") == "A"
    assert route(port, "suffix.route.invalid") == "B"
    assert route(port, "198.51.100.23") == "A"
    assert Counter(route(port, "unmatched.invalid") for _ in range(6)) == {"A": 3, "B": 3}
    assert Counter(route(port, "weighted.route.invalid") for _ in range(6)) == {"A": 4, "B": 2}
    print("PASS real routing: first-match, domain, CIDR, block, round-robin 3:3 and weighted 4:2")
    with connect(port, "manual.route.invalid") as established:
        assert exchange(established) == "B"
        api(controller, secret, "manual", "a")
        assert route(port, "manual.route.invalid") == "A"
        assert exchange(established) == "B", "manual selection changed an established flow"
    print("PASS selector controller changes only new flows; established flow stays pinned")
    before = len(b.payloads)
    rejected(port, "failure.route.invalid")
    assert len(b.payloads) == before, "failed payload was replayed across members"
    assert route(port, "failure.route.invalid") == "B"
    print("PASS failed round-robin member does not replay user payload; next flow advances")
    await_selected(controller, secret, "a")
    assert route(port, "auto.route.invalid") == "A"
    a.healthy = False
    await_selected(controller, secret, "b")
    assert route(port, "auto.route.invalid") == "B"
    assert a.probes and b.probes and not a.errors and not b.errors
    print("PASS URL-test chooses controlled faster member and switches after health failure")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, default=Path("dist/windows/client/blazecore.exe"))
    options = parser.parse_args()
    executable = options.executable.resolve(strict=True)
    assert hashlib.sha256(executable.read_bytes()).hexdigest().upper() == SHA256, "core pin mismatch"
    version = subprocess.run([str(executable), "-V"], capture_output=True, text=True,
                             creationflags=NO_WINDOW, timeout=5).stdout.strip()
    assert version == "BlazeCore " + VERSION, version
    with tempfile.TemporaryDirectory(prefix="blazer-balancer-contract-") as temporary:
        directory = Path(temporary)
        with upstream("A", 0.005) as a, upstream("B", 0.15) as b, socket.socket() as dead:
            dead.bind(("127.0.0.1", 0))  # Reserved but not listening: deterministic refusal.
            port, controller, secret = free_port(), free_port(), secrets.token_hex(24)
            document = config(port, controller, secret, a.server_address[1], b.server_address[1], dead.getsockname()[1])
            checked = core_check(executable, directory, document)
            assert checked.returncode == 0, checked.stdout + checked.stderr
            for kind in ("random", "fallback", "failover", "leastPing", "leastLoad"):
                invalid = json.loads(json.dumps(document))
                invalid[3]["type"] = kind
                assert core_check(executable, directory, invalid).returncode != 0, kind
            print(f"PASS pinned {VERSION}: native graph check; unsupported policy names rejected")
            with running_core(executable, directory, document):
                check_runtime(port, controller, secret, a, b)


if __name__ == "__main__":
    main()
