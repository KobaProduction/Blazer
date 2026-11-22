"""Packaged Windows ping settings and probe acceptance (Python stdlib only).

Run after packaging: python scripts/test-probe-settings.py [--online-https]
Uses isolated catalogs and loopback fixtures. Does not change Windows proxy/TUN.
The optional HTTPS case reaches Google's public connectivity-check endpoint.
"""

import argparse
import contextlib
import http.server
import os
from pathlib import Path
import re
import select
import socket
import socketserver
import subprocess
import tempfile
import threading
import time


HOST = "blazer-ping-fixture.invalid"
HTTPS_HOST = "connectivitycheck.gstatic.com"
HTTPS_URL = f"https://{HTTPS_HOST}/generate_204"
HEAD_PATH = "/custom/ping?configured=head"
NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)


def cli(executable, environment, *arguments, input_text=""):
    result = subprocess.run(
        [str(executable), "--cli", *arguments],
        input=input_text, text=True, capture_output=True, env=environment,
        creationflags=NO_WINDOW, timeout=40,
    )
    assert result.returncode == 0, (
        f"CLI {arguments[0]} failed: {result.stdout} {result.stderr}"
    )
    return result.stdout


def environment_for(executable, directory):
    environment = os.environ.copy()
    environment.update(
        BLAZER_DATA_DIR=str(directory), OTEL_SDK_DISABLED="true",
        BLAZER_BLAZECORE_PATH=str(executable.parent / "blazecore.exe"),
    )
    return environment


def import_socks(executable, environment, port):
    cli(executable, environment, "import",
        input_text=f"socks5://127.0.0.1:{port}#ping-acceptance-fixture\n")
    profiles = cli(executable, environment, "profiles")
    match = re.search(r"^profile=([0-9a-f-]+) ", profiles, re.MULTILINE)
    assert match, "the synthetic SOCKS profile was not imported"
    cli(executable, environment, "profile", "select", match[1])
    return match[1]


def set_settings(executable, environment, method, http_method, attempts, strategy, url, warmup=True):
    cli(executable, environment, "ping-settings", method, http_method, "5000",
        "warmup" if warmup else "cold", str(attempts), strategy, input_text=url + "\n")
    # A new process must read the same preferences from the protected catalog.
    output = cli(executable, environment, "ping-settings")
    actual = dict(line.split("=", 1) for line in output.splitlines() if "=" in line)
    expected = {
        "ping_method": method, "ping_http_method": http_method,
        "ping_timeout_ms": "5000", "ping_warmup": str(warmup).lower(),
        "ping_http_warmup_required": str(warmup or url.startswith("https://")).lower(),
        "ping_attempts": str(attempts), "ping_strategy": strategy, "ping_url": url,
    }
    assert all(actual.get(key) == value for key, value in expected.items()), (
        f"preferences were not persisted: {actual}"
    )


def test_profile(executable, environment, profile):
    output = cli(executable, environment, "profile", "test", profile)
    assert re.search(r"probe=reachable milliseconds=\d+", output), (
        f"probe did not report reachable: {output}"
    )


def read_exact(stream, count):
    result = bytearray()
    while len(result) < count:
        data = stream.recv(count - len(result))
        if not data:
            raise EOFError("SOCKS peer closed early")
        result.extend(data)
    return bytes(result)


class FixtureServer(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, handler):
        super().__init__(("127.0.0.1", 0), handler)
        self.connections = []
        self.requests = []
        self.errors = []
        self.http_port = None
        self.allow_https = False


@contextlib.contextmanager
def serving(handler):
    server = FixtureServer(handler)
    worker = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
    worker.start()
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=2)


class TcpFixture(socketserver.BaseRequestHandler):
    def handle(self):
        self.server.connections.append(self.client_address)
        self.request.settimeout(10)
        with contextlib.suppress(OSError):
            # TCP probes establish a connection and close it without payload.
            while self.request.recv(1024):
                pass


class HttpFixture(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_HEAD(self):
        self.server.requests.append(("HEAD", self.path, self.client_address))
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, *_):
        pass


class SocksFixture(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            self.request.settimeout(10)
            version, methods = read_exact(self.request, 2)
            assert version == 5
            read_exact(self.request, methods)
            self.request.sendall(b"\x05\x00")
            version, command, _, kind = read_exact(self.request, 4)
            assert (version, command) == (5, 1)
            if kind == 3:
                destination = read_exact(self.request, read_exact(self.request, 1)[0]).decode("ascii")
            elif kind == 1:
                destination = socket.inet_ntoa(read_exact(self.request, 4))
            elif kind == 4:
                destination = socket.inet_ntop(socket.AF_INET6, read_exact(self.request, 16))
            else:
                raise AssertionError("unexpected SOCKS address type")
            port = int.from_bytes(read_exact(self.request, 2), "big")
            if destination == HOST and port == 80:
                target = ("127.0.0.1", self.server.http_port)
            elif self.server.allow_https and destination == HTTPS_HOST and port == 443:
                target = (HTTPS_HOST, 443)
            else:
                raise AssertionError(f"fixture rejected destination {destination}:{port}")
            with socket.create_connection(target, timeout=10) as remote:
                self.server.connections.append((destination, port))
                self.request.sendall(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00")
                self.forward(remote)
        except (OSError, EOFError):
            pass  # Peer shutdown is expected after the temporary engine stops.
        except AssertionError as error:
            self.server.errors.append(str(error))

    def forward(self, remote):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            readable, _, _ = select.select([self.request, remote], [], [], 0.25)
            for source in readable:
                data = source.recv(65536)
                if not data:
                    return
                (remote if source is self.request else self.request).sendall(data)


def wait_for_count(values, expected):
    deadline = time.monotonic() + 2
    while len(values) < expected and time.monotonic() < deadline:
        time.sleep(0.01)
    assert len(values) == expected, f"expected {expected} connections, observed {len(values)}"


def offline_checks(executable, directory):
    icmp = environment_for(executable, directory / "icmp")
    default = cli(executable, icmp, "ping-settings")
    assert "ping_method=auto" in default and "ping_warmup=true" in default
    assert "ping_http_method=head" in default and "ping_attempts=2" in default
    assert "ping_http_warmup_required=true" in default
    profile = import_socks(executable, icmp, 9)
    set_settings(executable, icmp, "icmp", "get", 2, "median", HTTPS_URL)
    test_profile(executable, icmp, profile)
    print("PASS persisted preferences and native Windows ICMP")

    with serving(TcpFixture) as tcp:
        environment = environment_for(executable, directory / "tcp")
        profile = import_socks(executable, environment, tcp.server_address[1])
        set_settings(executable, environment, "tcp", "get", 3, "average", HTTPS_URL)
        test_profile(executable, environment, profile)
        wait_for_count(tcp.connections, 4)
    print("PASS TCP warmup plus three measured connections")

    proxy_http_check(executable, directory)


def proxy_http_check(executable, directory):
    with serving(HttpFixture) as http, serving(SocksFixture) as socks:
        socks.http_port = http.server_address[1]
        environment = environment_for(executable, directory / "proxy")
        profile = import_socks(executable, environment, socks.server_address[1])
        url = f"http://{HOST}{HEAD_PATH}"
        set_settings(executable, environment, "proxy", "head", 2, "min", url)
        try:
            test_profile(executable, environment, profile)
        except AssertionError:
            print(f"HTTP fixture diagnostics: errors={socks.errors}, requests={http.requests}, connections={socks.connections}")
            raise
        assert not socks.errors, socks.errors
        assert len(http.requests) == 3, f"expected warmup plus two requests: {http.requests}"
        assert all(method == "HEAD" and path == HEAD_PATH for method, path, _ in http.requests)
        assert len({peer for _, _, peer in http.requests}) == 1, "HTTP connection was not reused"
        assert socks.connections == [(HOST, 80)], f"proxy connection was not reused: {socks.connections}"
    print("PASS imported profile, custom HEAD URL and one warmed proxy connection")


def online_https_check(executable, directory):
    with serving(SocksFixture) as socks:
        socks.allow_https = True
        environment = environment_for(executable, directory / "https")
        profile = import_socks(executable, environment, socks.server_address[1])
        set_settings(executable, environment, "proxy", "head", 2, "last", HTTPS_URL, warmup=False)
        try:
            test_profile(executable, environment, profile)
        except AssertionError:
            print(f"HTTPS fixture diagnostics: errors={socks.errors}, connections={socks.connections}")
            raise
        assert not socks.errors, socks.errors
        assert socks.connections == [(HTTPS_HOST, 443)], (
            f"validated TLS connection was not reused: {socks.connections}"
        )
    print("PASS public HTTPS HEAD with mandatory warmup despite cold preference and TLS connection reuse")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, default=Path("dist/windows/client/blazer.exe"))
    parser.add_argument("--online-https", action="store_true")
    parser.add_argument("--case", choices=("all", "http", "https"), default="all")
    options = parser.parse_args()
    if os.name != "nt":
        parser.error("this acceptance profile requires Windows")
    executable = options.executable.resolve(strict=True)
    assert (executable.parent / "blazecore.exe").is_file(), "packaged BlazeCore is missing"
    with tempfile.TemporaryDirectory(prefix="blazer-ping-acceptance-") as temporary:
        directory = Path(temporary)
        if options.case == "all":
            offline_checks(executable, directory)
        elif options.case == "http":
            proxy_http_check(executable, directory)
        if options.online_https or options.case == "https":
            online_https_check(executable, directory)


if __name__ == "__main__":
    main()
