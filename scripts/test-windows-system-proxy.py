"""Opt-in Windows system-proxy acceptance test for the packaged Blazer executable.

Temporarily changes the current user's proxy. Uses an isolated catalog, local
SOCKS/HTTP/TLS fixtures and WinINet PRECONFIG requests, with emergency rollback.
Run after packaging: python scripts/test-windows-system-proxy.py
"""

import argparse
import contextlib
import ctypes
from ctypes import wintypes
import http.server
import os
from pathlib import Path
import queue
import re
import select
import shutil
import socket
import socketserver
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import winreg


INTERNET_SETTINGS = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings"
VALUES = {
    "": ("ProxyEnable", "ProxyServer", "ProxyOverride", "AutoConfigURL", "AutoDetect"),
    r"\Connections": ("DefaultConnectionSettings", "SavedLegacySettings"),
}
NO_WINDOW = subprocess.CREATE_NO_WINDOW
PAYLOAD = b"blazer-system-proxy-fixture"
HOST = "blazer-check.invalid"


def snapshot():
    result = {}
    for suffix, names in VALUES.items():
        for name in names:
            try:
                with winreg.OpenKey(winreg.HKEY_CURRENT_USER, INTERNET_SETTINGS + suffix) as key:
                    result[(suffix, name)] = winreg.QueryValueEx(key, name)
            except FileNotFoundError:
                result[(suffix, name)] = None
    return result


def emergency_restore(saved):
    for (suffix, name), value in saved.items():
        with winreg.CreateKeyEx(
            winreg.HKEY_CURRENT_USER, INTERNET_SETTINGS + suffix, access=winreg.KEY_SET_VALUE
        ) as key:
            if value is None:
                with contextlib.suppress(FileNotFoundError):
                    winreg.DeleteValue(key, name)
            else:
                winreg.SetValueEx(key, name, 0, value[1], value[0])
    wininet = ctypes.WinDLL("wininet", use_last_error=True)
    for option in (39, 37):  # INTERNET_OPTION_SETTINGS_CHANGED, REFRESH
        wininet.InternetSetOptionW(None, option, None, 0)


def wait_for(predicate, description, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError(description)


def assert_restored(saved):
    wait_for(lambda: snapshot() == saved, "original Windows proxy settings were not restored")


def read_exact(stream, count):
    result = bytearray()
    while len(result) < count:
        piece = stream.recv(count - len(result))
        if not piece:
            raise EOFError("fixture peer closed the connection")
        result.extend(piece)
    return bytes(result)


class HttpFixture(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", str(len(PAYLOAD)))
        self.end_headers()
        self.wfile.write(PAYLOAD)

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
                destination = read_exact(self.request, read_exact(self.request, 1)[0]).decode()
            elif kind == 1:
                destination = socket.inet_ntoa(read_exact(self.request, 4))
            else:
                raise AssertionError("unexpected SOCKS destination address family")
            port = int.from_bytes(read_exact(self.request, 2), "big")
            assert destination == HOST, "fixture rejects traffic outside the test hostname"
            target = self.server.targets[port]
            with socket.create_connection(("127.0.0.1", target), timeout=10) as remote:
                self.server.connections.append(port)
                self.request.sendall(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00")
                while True:
                    readable, _, _ = select.select([self.request, remote], [], [], 10)
                    if not readable:
                        return
                    for source in readable:
                        data = source.recv(65536)
                        if not data:
                            return
                        (remote if source is self.request else self.request).sendall(data)
        except (OSError, EOFError, AssertionError, KeyError):
            return


class SocksServer(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True


def start_server(server):
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def create_fixtures(directory):
    openssl = shutil.which("openssl") or r"C:\Program Files\Git\usr\bin\openssl.exe"
    certificate, private_key = directory / "fixture.pem", directory / "fixture.key"
    subprocess.run(
        [openssl, "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
         "-subj", f"/CN={HOST}", "-keyout", str(private_key), "-out", str(certificate)],
        check=True, capture_output=True, creationflags=NO_WINDOW, timeout=20,
    )
    plain = http.server.ThreadingHTTPServer(("127.0.0.1", 0), HttpFixture)
    secure = http.server.ThreadingHTTPServer(("127.0.0.1", 0), HttpFixture)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(certificate, private_key)
    secure.socket = context.wrap_socket(secure.socket, server_side=True)
    upstream = SocksServer(("127.0.0.1", 0), SocksFixture)
    upstream.targets = {80: plain.server_port, 443: secure.server_port}
    upstream.connections = []
    return [start_server(server) for server in (plain, secure, upstream)]


def native_request(secure):
    """Use the Windows user's actual proxy; no explicit proxy is passed."""
    api = ctypes.WinDLL("wininet", use_last_error=True)
    api.InternetOpenW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.LPCWSTR,
                                 wintypes.LPCWSTR, wintypes.DWORD]
    api.InternetOpenW.restype = wintypes.HANDLE
    api.InternetConnectW.argtypes = [wintypes.HANDLE, wintypes.LPCWSTR, wintypes.WORD,
                                    wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD,
                                    wintypes.DWORD, ctypes.c_size_t]
    api.InternetConnectW.restype = wintypes.HANDLE
    api.HttpOpenRequestW.argtypes = [wintypes.HANDLE, wintypes.LPCWSTR, wintypes.LPCWSTR,
                                    wintypes.LPCWSTR, wintypes.LPCWSTR, ctypes.c_void_p,
                                    wintypes.DWORD, ctypes.c_size_t]
    api.HttpOpenRequestW.restype = wintypes.HANDLE
    api.InternetSetOptionW.argtypes = [wintypes.HANDLE, wintypes.DWORD,
                                      ctypes.c_void_p, wintypes.DWORD]
    api.HttpSendRequestW.argtypes = [wintypes.HANDLE, wintypes.LPCWSTR, wintypes.DWORD,
                                    ctypes.c_void_p, wintypes.DWORD]
    api.InternetReadFile.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD,
                                    ctypes.POINTER(wintypes.DWORD)]
    api.InternetCloseHandle.argtypes = [wintypes.HANDLE]
    handles = []
    try:
        session = api.InternetOpenW("BlazerSystemProxySmoke", 0, None, None, 0)
        assert session, f"InternetOpen failed: {ctypes.get_last_error()}"
        handles.append(session)
        timeout = wintypes.DWORD(7000)
        for option in (2, 5, 6):  # connect, send and receive timeouts
            assert api.InternetSetOptionW(session, option, ctypes.byref(timeout), 4)
        connection = api.InternetConnectW(session, HOST, 443 if secure else 80,
                                          None, None, 3, 0, 0)
        assert connection, f"InternetConnect failed: {ctypes.get_last_error()}"
        handles.append(connection)
        flags = 0x80000000 | 0x04000000 | (0x00800000 if secure else 0)
        request = api.HttpOpenRequestW(connection, "GET", "/test", None, None, None, flags, 0)
        assert request, f"HttpOpenRequest failed: {ctypes.get_last_error()}"
        handles.append(request)
        if secure:
            # Only the local self-signed fixture ignores trust errors.
            security_flags = wintypes.DWORD(0x100 | 0x1000 | 0x2000)
            assert api.InternetSetOptionW(request, 31, ctypes.byref(security_flags), 4)
        assert api.HttpSendRequestW(request, None, 0, None, 0), (
            f"WinINet {'HTTPS' if secure else 'HTTP'} failed: {ctypes.get_last_error()}"
        )
        buffer, count = ctypes.create_string_buffer(4096), wintypes.DWORD()
        assert api.InternetReadFile(request, buffer, len(buffer), ctypes.byref(count))
        assert buffer.raw[:count.value] == PAYLOAD, "fixture response did not traverse the proxy"
    finally:
        for handle in reversed(handles):
            api.InternetCloseHandle(handle)


def cli(executable, environment, *arguments, input_text=""):
    result = subprocess.run([str(executable), "--cli", *arguments], input=input_text,
                            text=True, capture_output=True, env=environment,
                            creationflags=NO_WINDOW, timeout=25)
    assert result.returncode == 0, f"CLI {arguments[0]} failed: {result.stdout} {result.stderr}"
    return result.stdout


def prepare_catalog(executable, directory, upstream_port):
    environment = os.environ.copy()
    environment.update(BLAZER_DATA_DIR=str(directory), OTEL_SDK_DISABLED="true",
                       BLAZER_BLAZECORE_PATH=str(executable.parent / "blazecore.exe"))
    cli(executable, environment, "import",
        input_text=f"socks5://127.0.0.1:{upstream_port}#system-proxy-fixture\n")
    profiles = cli(executable, environment, "profiles")
    match = re.search(r"^profile=([0-9a-f-]+) ", profiles, re.MULTILINE)
    assert match, "fixture profile was not imported"
    cli(executable, environment, "profile", "select", match[1])
    return environment


def launch_connection(executable, environment):
    process = subprocess.Popen([str(executable), "--cli", "system-proxy"],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, env=environment,
                               creationflags=NO_WINDOW)
    lines = queue.Queue()

    def read_lines():
        for line in process.stdout:
            lines.put(line.rstrip())
        lines.put(None)

    threading.Thread(target=read_lines, daemon=True).start()
    return process, lines


def await_connected(process, lines):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            line = lines.get(timeout=0.1)
        except queue.Empty:
            continue
        assert line is not None, "system-proxy process exited before readiness"
        assert not line.startswith("error="), f"system-proxy rejected start: {line}"
        if line.startswith("local_proxy="):
            return
    raise AssertionError("system-proxy did not report readiness")


def stop(process, force=False):
    if process.poll() is None:
        if force:
            process.kill()
        else:
            process.stdin.write("\n")
            process.stdin.flush()
        process.wait(timeout=15)
    for stream in (process.stdin, process.stdout):
        if stream:
            stream.close()


def child_process_id(parent_pid, executable_name):
    class ProcessEntry(ctypes.Structure):
        _fields_ = [("dwSize", wintypes.DWORD), ("cntUsage", wintypes.DWORD),
                    ("th32ProcessID", wintypes.DWORD), ("th32DefaultHeapID", ctypes.c_size_t),
                    ("th32ModuleID", wintypes.DWORD), ("cntThreads", wintypes.DWORD),
                    ("th32ParentProcessID", wintypes.DWORD), ("pcPriClassBase", wintypes.LONG),
                    ("dwFlags", wintypes.DWORD), ("szExeFile", wintypes.WCHAR * 260)]

    api = ctypes.WinDLL("kernel32", use_last_error=True)
    api.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    api.Process32FirstW.argtypes = api.Process32NextW.argtypes = [wintypes.HANDLE,
                                                               ctypes.POINTER(ProcessEntry)]
    api.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = api.CreateToolhelp32Snapshot(2, 0)
    assert handle != wintypes.HANDLE(-1).value
    entry = ProcessEntry()
    entry.dwSize = ctypes.sizeof(entry)
    try:
        available = api.Process32FirstW(handle, ctypes.byref(entry))
        while available:
            if (entry.th32ParentProcessID == parent_pid
                    and entry.szExeFile.lower() == executable_name.lower()):
                return entry.th32ProcessID
            available = api.Process32NextW(handle, ctypes.byref(entry))
        raise AssertionError(f"the connected CLI's {executable_name} child was not found")
    finally:
        api.CloseHandle(handle)


def terminate_process(pid):
    api = ctypes.WinDLL("kernel32", use_last_error=True)
    api.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    api.OpenProcess.restype = wintypes.HANDLE
    api.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
    api.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    api.CloseHandle.argtypes = [wintypes.HANDLE]
    process = api.OpenProcess(0x00100001, False, pid)
    assert process, "cannot open test child process"
    try:
        assert api.TerminateProcess(process, 1), "cannot stop test child process"
        assert api.WaitForSingleObject(process, 5000) == 0, "test child did not terminate"
    finally:
        api.CloseHandle(process)


def kill_process_tree_without_cleanup(process, guardian_pid):
    # Suspend the parent first, eliminating a race with its 400 ms recovery poll.
    api = ctypes.WinDLL("kernel32", use_last_error=True)
    api.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    api.OpenProcess.restype = wintypes.HANDLE
    api.CloseHandle.argtypes = [wintypes.HANDLE]
    native = ctypes.WinDLL("ntdll", use_last_error=True)
    native.NtSuspendProcess.argtypes = [wintypes.HANDLE]
    native.NtSuspendProcess.restype = wintypes.LONG
    parent = api.OpenProcess(0x0800, False, process.pid)
    assert parent, "cannot open test parent for suspended crash simulation"
    try:
        assert native.NtSuspendProcess(parent) >= 0, "cannot suspend test parent"
        try:
            terminate_process(guardian_pid)
        finally:
            process.kill()
            process.wait(timeout=15)
    finally:
        api.CloseHandle(parent)


def apply_external_change():
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, INTERNET_SETTINGS,
                        access=winreg.KEY_SET_VALUE) as key:
        winreg.SetValueEx(key, "ProxyEnable", 0, winreg.REG_DWORD, 0)
        winreg.SetValueEx(key, "ProxyServer", 0, winreg.REG_SZ, "external-test.invalid:8080")
    return snapshot()


def verify_traffic(upstream):
    previous = len(upstream.connections)
    for protocol in ("http", "https"):
        result = subprocess.run([sys.executable, __file__, "--request", protocol],
                                capture_output=True, text=True, creationflags=NO_WINDOW, timeout=15)
        assert result.returncode == 0, f"native {protocol.upper()} failed: {result.stderr}"
    assert sorted(upstream.connections[previous:]) == [80, 443], (
        "HTTP and HTTPS did not both traverse the selected SOCKS upstream"
    )


def acceptance(executable):
    saved = snapshot()
    processes, servers = [], []
    temporary = tempfile.TemporaryDirectory(prefix="blazer-system-proxy-")
    try:
        directory = Path(temporary.name)
        servers = create_fixtures(directory)
        upstream = servers[-1]
        environment = prepare_catalog(executable, directory / "catalog", upstream.server_address[1])
        invalid = environment | {"BLAZER_BLAZECORE_PATH": str(directory / "absent.exe")}
        failed, _ = launch_connection(executable, invalid)
        processes.append(failed)
        assert failed.wait(timeout=20) != 0, "invalid engine unexpectedly started"
        assert_restored(saved)
        print("PASS invalid engine leaves system settings unchanged", flush=True)

        for scenario in ("graceful", "parent-kill", "engine-kill", "conflict",
                         "guardian-kill", "tree-kill-recovery", "external-change"):
            process, lines = launch_connection(executable, environment)
            processes.append(process)
            await_connected(process, lines)
            assert snapshot() != saved, "SystemProxy did not change Windows settings"
            verify_traffic(upstream)
            if scenario == "conflict":
                other = prepare_catalog(executable, directory / "other", upstream.server_address[1])
                active = snapshot()
                second, _ = launch_connection(executable, other)
                processes.append(second)
                assert second.wait(timeout=15) != 0, "second process stole active system proxy"
                assert snapshot() == active, "second process changed first process's settings"
            if scenario in ("engine-kill", "guardian-kill"):
                executable_name = "engine.exe" if scenario == "engine-kill" else executable.name
                terminate_process(child_process_id(process.pid, executable_name))
                process.wait(timeout=15)
            elif scenario == "tree-kill-recovery":
                active = snapshot()
                guardian_pid = child_process_id(process.pid, executable.name)
                kill_process_tree_without_cleanup(process, guardian_pid)
                assert snapshot() == active, "crash simulation unexpectedly cleaned settings"
                cli(executable, environment, "status")
            elif scenario == "external-change":
                external = apply_external_change()
                stop(process)
                assert snapshot() == external, "disconnect overwrote the user's newer settings"
                emergency_restore(saved)
            else:
                stop(process, force=scenario == "parent-kill")
            assert_restored(saved)
            print(f"PASS {scenario}: native HTTP + HTTPS; exact proxy/PAC/bypass restoration",
                  flush=True)
    finally:
        for process in processes:
            with contextlib.suppress(Exception):
                stop(process, force=True)
        for server in reversed(servers):
            server.shutdown()
            server.server_close()
        if snapshot() != saved:
            emergency_restore(saved)
            assert snapshot() == saved, "emergency restoration failed"
            print("Emergency rollback restored original proxy settings", file=sys.stderr)
        temporary.cleanup()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path,
                        default=Path(__file__).resolve().parents[1] / "dist/windows/client/blazer.exe")
    parser.add_argument("--request", choices=("http", "https"), help=argparse.SUPPRESS)
    arguments = parser.parse_args()
    if arguments.request:
        native_request(arguments.request == "https")
    else:
        acceptance(arguments.exe.resolve(strict=True))


if __name__ == "__main__":
    main()
