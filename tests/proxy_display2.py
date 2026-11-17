"""Verify that a stopped local proxy does not fall back to a direct web request."""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from json import dumps
from pathlib import Path
from subprocess import run
from socketserver import StreamRequestHandler, ThreadingTCPServer
from threading import Thread

from smoke_display2 import ROOT, test_profile


class Handler(BaseHTTPRequestHandler):
    direct = []
    proxied = []

    def do_GET(self):
        if self.path.startswith('http://'):
            self.proxied.append(self.path)
        else:
            self.direct.append(self.path)
        body = b'<html><title>Local route test</title>OK</html>'
        self.send_response(200)
        self.send_header('Content-Type', 'text/html')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def launch(profile, target):
    result = run(
        [ROOT / 'target/debug/rust-browser.exe', '--second-monitor', '--smoke',
         f'--profile={profile}', f'--url={target}'],
        check=True, capture_output=True, text=True, timeout=25,
    )
    print(result.stdout, end="")
    assert 'DISPLAY2' in result.stdout, result.stderr


def main():
    with test_profile() as profile:
        with ThreadingHTTPServer(('127.0.0.1', 0), Handler) as origin:
            Thread(target=origin.serve_forever, daemon=True).start()
            try:
                target = f'http://127.0.0.1:{origin.server_port}/route-test'
                with ThreadingHTTPServer(('127.0.0.1', 0), Handler) as proxy:
                    endpoint = f'http://127.0.0.1:{proxy.server_port}'
                    Path(profile, 'preferences.json').write_text(dumps({
                        'network': {'locked': True, 'proxy': endpoint},
                    }))
                    Thread(target=proxy.serve_forever, daemon=True).start()
                    try:
                        launch(profile, target)
                        assert target in Handler.proxied, (Handler.proxied, Handler.direct)
                        assert not Handler.direct, Handler.direct
                    finally:
                        proxy.shutdown()
                launch(profile, target + '-proxy-down')
                assert not Handler.direct, Handler.direct
                print('Browser used the specified proxy, then made no direct request after it stopped.')
            finally:
                origin.shutdown()


class SocksHandler(StreamRequestHandler):
    hosts = []
    requests = []

    def handle(self):
        self.request.settimeout(4)
        hello = self.rfile.read(2)
        if len(hello) != 2 or hello[0] != 5:
            return
        self.rfile.read(hello[1])
        self.wfile.write(b"\x05\x00")
        command = self.rfile.read(4)
        if len(command) != 4 or command[:3] != b"\x05\x01\x00":
            return
        if command[3] != 3:
            return
        length = self.rfile.read(1)
        name = self.rfile.read(length[0])
        self.rfile.read(2)
        self.hosts.append(name)
        self.wfile.write(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00")
        first_line = self.rfile.readline(2048)
        if first_line.startswith(b"GET "):
            self.requests.append(first_line)
            for line in iter(self.rfile.readline, b""):
                if line in (b"\n", b"\r\n"):
                    break
            self.wfile.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")


def check_socks():
    with test_profile() as profile, ThreadingTCPServer(('127.0.0.1', 0), SocksHandler) as proxy:
        proxy.daemon_threads = True
        endpoint = f'socks5://127.0.0.1:{proxy.server_address[1]}'
        Path(profile, 'preferences.json').write_text(dumps({
            'network': {'locked': True, 'proxy': endpoint},
        }))
        Thread(target=proxy.serve_forever, daemon=True).start()
        try:
            launch(profile, 'http://orbital-route.test/dns-check')
            assert b'orbital-route.test' in SocksHandler.hosts, SocksHandler.hosts
            assert any(b'/dns-check' in line for line in SocksHandler.requests), SocksHandler.requests
            print('SOCKS5 received the unresolved destination hostname.')
        finally:
            proxy.shutdown()

if __name__ == '__main__':
    main()
    check_socks()
