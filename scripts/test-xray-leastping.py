"""Packaged Blazer leastPing/observatory acceptance through isolated loopback fixtures.

No OS proxy/TUN changes, user catalogs, remote HTTP requests or core changes.
"""

import argparse
import ctypes
from ctypes import wintypes
import importlib.util
import json
from pathlib import Path
import re
import socket
import tempfile
import time


spec = importlib.util.spec_from_file_location(
    "blazer_balancer_pipeline", Path(__file__).with_name("test-xray-balancer.py")
)
pipeline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pipeline)
fixtures = pipeline.fixtures
DESTINATION = "leastping.fixture.invalid"


def document(a, b, c, fallback=True, subjects=None):
    def outbound(tag, server):
        return {"tag": tag, "protocol": "socks", "settings": {
            "servers": [{"address": "127.0.0.1", "port": server.server_address[1]}]
        }}

    balancer = {"tag": "fastest", "selector": ["candidate-"],
                "strategy": {"type": "leastPing"}}
    if fallback:
        balancer["fallbackTag"] = "global-default"
    return {
        "remarks": "Shared observatory fixture",
        # The global default intentionally differs from the first candidate A.
        "outbounds": [outbound("global-default", c), outbound("candidate-a", a),
                      outbound("candidate-b", b)],
        "routing": {"domainStrategy": "AsIs", "balancers": [balancer],
                    "rules": [{"type": "field", "network": "tcp", "balancerTag": "fastest"}]},
        "observatory": {"subjectSelector": subjects or ["candidate-"],
                        "probeURL": "http://health.fixture.invalid/check",
                        "probeInterval": "200ms", "enableConcurrency": True},
    }


def wait_for(predicate, description, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError(description)


def await_route(port, expected, description):
    observed = []

    def selected():
        try:
            label = fixtures.route(port, DESTINATION)
        except (OSError, EOFError):
            return False
        observed.append(label)
        return label == expected

    try:
        wait_for(selected, description)
    except AssertionError:
        raise AssertionError(f"{description}: recent routes {observed[-10:]}") from None


def reset_health(a, b):
    a.healthy = b.healthy = True
    a.delay, b.delay = 0.005, 0.150
    a.probe_status = b.probe_status = 204


def await_first_round(a, b, previous):
    wait_for(lambda: a.probes >= previous[0] + 2 and b.probes >= previous[1] + 2,
             "shared observer did not complete a first round for both subjects")


def import_manual(executable, directory, source):
    environment = pipeline.environment_for(executable, directory)
    pipeline.cli(executable, environment, "import", input_text=json.dumps(source))
    pipeline.single_graph(executable, environment)
    return environment


def check_health_transitions(executable, environment, a, b):
    reset_health(a, b)
    previous = (a.probes, b.probes)
    with pipeline.running_proxy(executable, environment) as port:
        await_first_round(a, b, previous)
        await_route(port, "A", "leastPing did not select the faster healthy subject")
        with fixtures.connect(port, DESTINATION) as established:
            assert fixtures.exchange(established) == "A"
            # Xray considers receipt of any HTTP status a successful observation.
            a.probe_status = 503
            before = a.probes
            wait_for(lambda: a.probes >= before + 2, "GET status fixture was not observed")
            await_route(port, "A", "an HTTP 503 response was incorrectly classified as dead")
            a.healthy = False
            await_route(port, "B", "failed A remained selected instead of healthy B")
            assert fixtures.exchange(established) == "A", "existing flow changed its selected server"
            b.healthy = False
            await_route(port, "C", "all failed subjects did not switch to explicit fallback C")
            a.healthy, a.probe_status = True, 204
            await_route(port, "A", "recovered A did not replace fallback C")
            assert fixtures.exchange(established) == "A"
    assert a.probe_methods and b.probe_methods
    assert set(a.probe_methods + b.probe_methods) == {"GET"}, "observatory must use GET"
    print("PASS leastPing: fastest A; HTTP 503 alive; fail A->B; both dead->C; recover A; held flow stays A",
          flush=True)


def check_subscription(executable, directory, a, b, c):
    environment = pipeline.environment_for(executable, directory)
    source = document(a, b, c)
    with pipeline.subscription(source) as server:
        pipeline.cli(executable, environment, "subscription", "add", "LeastPing fixture",
                     input_text=f"http://127.0.0.1:{server.server_address[1]}/subscription\n")
        original = pipeline.single_graph(executable, environment)
        subscriptions = pipeline.cli(executable, environment, "subscriptions")
        match = re.search(r"^subscription=([0-9a-f-]+) profiles=1 ", subscriptions, re.MULTILINE)
        assert match, "subscription did not retain one complete graph"
        server.encoded = True
        pipeline.cli(executable, environment, "subscription", "refresh", match[1])
        assert pipeline.single_graph(executable, environment) == original, "Base64 refresh changed graph identity"
        assert server.requests == 2
        check_health_transitions(executable, environment, a, b)
    print("PASS HTTP/Base64 subscription -> protected graph -> new CLI process with stable UUID", flush=True)


def check_global_default(executable, directory, a, b, c):
    reset_health(a, b)
    a.healthy = b.healthy = False
    environment = import_manual(executable, directory, document(a, b, c, fallback=False))
    previous = (a.probes, b.probes)
    with pipeline.running_proxy(executable, environment) as port:
        await_first_round(a, b, previous)
        await_route(port, "C", "no-fallback graph did not use its global default C")
        assert [fixtures.route(port, DESTINATION) for _ in range(3)] == ["C"] * 3
    print("PASS all-dead without fallbackTag uses global default C, excluding first candidate A", flush=True)


def check_unobserved_subject(executable, directory, a, b, c):
    reset_health(a, b)
    source = document(a, b, c, subjects=["candidate-b"])
    environment = import_manual(executable, directory, source)
    initial_a, initial_b = a.probes, b.probes
    with pipeline.running_proxy(executable, environment) as port:
        wait_for(lambda: b.probes >= initial_b + 2, "selected observer subject B was not probed")
        await_route(port, "B", "unobserved fast candidate A was incorrectly selected")
        assert a.probes == initial_a, "subjectSelector did not restrict shared observation"
        b.healthy = False
        await_route(port, "C", "unknown A was used instead of fallback C")
    print("PASS shared subjectSelector excludes unobserved candidate A", flush=True)


def check_pending_shutdown(executable, directory, a, b, c):
    reset_health(a, b)
    a.delay = b.delay = 20
    environment = import_manual(executable, directory, document(a, b, c))
    previous = (a.probes, b.probes)
    with pipeline.running_proxy(executable, environment):
        wait_for(lambda: a.probes > previous[0] and b.probes > previous[1],
                 "pending HTTP observations did not start")
        started = time.monotonic()
    elapsed = time.monotonic() - started
    assert elapsed < 3, f"shutdown did not promptly cancel pending observations: {elapsed:.2f}s"
    print("PASS shutdown cancels pending HTTP observations within 3 seconds", flush=True)


def listening_processes():
    class Row(ctypes.Structure):
        _fields_ = [("state", wintypes.DWORD), ("local_address", wintypes.DWORD),
                    ("local_port", wintypes.DWORD), ("remote_address", wintypes.DWORD),
                    ("remote_port", wintypes.DWORD), ("pid", wintypes.DWORD)]

    api = ctypes.WinDLL("iphlpapi")
    api.GetExtendedTcpTable.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.DWORD),
                                       wintypes.BOOL, wintypes.ULONG, ctypes.c_int, wintypes.ULONG]
    size = wintypes.DWORD()
    assert api.GetExtendedTcpTable(None, ctypes.byref(size), False, socket.AF_INET, 5, 0) == 122
    buffer = ctypes.create_string_buffer(size.value)
    assert api.GetExtendedTcpTable(buffer, ctypes.byref(size), False, socket.AF_INET, 5, 0) == 0
    count = wintypes.DWORD.from_buffer(buffer).value
    rows = (Row * count).from_buffer(buffer, ctypes.sizeof(wintypes.DWORD))
    result = {}
    for row in rows:
        if row.state == 2:
            result.setdefault(row.pid, set()).add(socket.ntohs(row.local_port & 0xffff))
    return result


def process_running(pid):
    api = ctypes.WinDLL("kernel32", use_last_error=True)
    api.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    api.OpenProcess.restype = wintypes.HANDLE
    api.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    api.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = api.OpenProcess(0x00100000, False, pid)
    if not handle:
        return False
    try:
        return api.WaitForSingleObject(handle, 0) == 258
    finally:
        api.CloseHandle(handle)


def check_serial_and_cleanup(executable, directory, a, b, c):
    reset_health(a, b)
    source = document(a, b, c)
    source["observatory"]["enableConcurrency"] = False
    environment = import_manual(executable, directory, source)
    initial_a, initial_b = len(a.probe_times), len(b.probe_times)
    before = listening_processes()
    with pipeline.running_proxy(executable, environment) as port:
        wait_for(lambda: len(a.probe_times) >= initial_a + 2 and len(b.probe_times) >= initial_b + 2,
                 "serial observer did not complete two rounds")
        await_route(port, "A", "serial observer did not choose lowest healthy delay")
        arrivals = sorted([(moment, "A") for moment in a.probe_times[initial_a:]]
                          + [(moment, "B") for moment in b.probe_times[initial_b:]])
        assert [label for _, label in arrivals[:4]] == ["A", "B", "A", "B"], arrivals
        assert all(right[0] - left[0] >= 0.18 for left, right in zip(arrivals[:3], arrivals[1:4])), (
            "serial probes did not retain the configured 200ms pause after each subject"
        )
        a.healthy = False
        await_route(port, "B", "serial observer retained stale A after its failed observation")
        active = listening_processes()
        owners = [(pid, ports) for pid, ports in active.items() if port in ports and port not in before.get(pid, set())]
        assert len(owners) == 1, "cannot identify the test engine's listener owner"
        engine_pid, owned_ports = owners[0]
        assert len(owned_ports) >= 4, "missing controller or fixed-outbound observation listeners"
    wait_for(lambda: not process_running(engine_pid) and engine_pid not in listening_processes(),
             "engine or private observer/control listeners remained after stop", timeout=5)
    for owned_port in owned_ports:
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", owned_port))
    print("PASS serial GET order A B A B with configured per-subject pause; fail A->B; engine and all public/private ports released",
          flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, default=Path("dist/windows/client/blazer.exe"))
    parser.add_argument("--focused-lifecycle", action="store_true",
                        help="Only check serial scheduling and all engine listener cleanup")
    arguments = parser.parse_args()
    executable = arguments.executable.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="blazer-leastping-") as temporary:
        directory = Path(temporary)
        with fixtures.upstream("A", 0.005) as a, fixtures.upstream("B", 0.150) as b, fixtures.upstream("C", 0) as c:
            a.socket_timeout = b.socket_timeout = c.socket_timeout = 30
            if arguments.focused_lifecycle:
                check_serial_and_cleanup(executable, directory / "serial", a, b, c)
                return
            manual = import_manual(executable, directory / "manual", document(a, b, c))
            check_health_transitions(executable, manual, a, b)
            check_subscription(executable, directory / "subscription", a, b, c)
            check_global_default(executable, directory / "global", a, b, c)
            check_unobserved_subject(executable, directory / "subject", a, b, c)
            check_pending_shutdown(executable, directory / "shutdown", a, b, c)
            check_serial_and_cleanup(executable, directory / "serial", a, b, c)
            assert not a.errors and not b.errors and not c.errors, "fixture protocol mismatch"


if __name__ == "__main__":
    main()
