"""Exercise a real WebView2 window and a persistent profile on DISPLAY2."""

from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from subprocess import run
from tempfile import TemporaryDirectory
from time import sleep
from threading import Thread

ROOT = Path(__file__).resolve().parents[1]
HTML = b"""<!doctype html>
<title>Rust Browser smoke test</title>
<video muted autoplay></video><canvas width="320" height="180" hidden></canvas>
<script>
const canvas = document.querySelector('canvas');
const video = document.querySelector('video');
const start = performance.now();
video.srcObject = canvas.captureStream(30);
const context = canvas.getContext('2d');
setInterval(() => {
  context.fillStyle = performance.now() % 800 < 400 ? '#34bb9a' : '#165b75';
  context.fillRect(0, 0, 320, 180);
}, 33);
video.requestVideoFrameCallback(() =>
  fetch('/first-frame?ms=' + Math.round(performance.now() - start)));
</script>"""


class Handler(BaseHTTPRequestHandler):
    visits = []
    frames = []

    def do_GET(self):
        if self.path == '/':
            self.visits.append('test_session=ok' in (self.headers.get('Cookie') or ''))
            self.send_response(200)
            self.send_header('Content-Type', 'text/html')
            self.send_header('Set-Cookie', 'test_session=ok; Max-Age=3600; HttpOnly; SameSite=Lax')
            self.send_header('Content-Length', str(len(HTML)))
            self.end_headers()
            self.wfile.write(HTML)
        elif self.path.startswith('/first-frame?ms='):
            self.frames.append(int(self.path.rpartition('=')[2]))
            self.send_response(204)
            self.end_headers()
        else:
            self.send_error(404)

    def log_message(self, *args):
        pass


@contextmanager
def test_profile():
    temporary = TemporaryDirectory(prefix='profile-smoke-', dir=ROOT)
    try:
        yield temporary.name
    finally:
        # WebView2 child processes can briefly retain files after the host exits.
        for attempt in range(20):
            try:
                temporary.cleanup()
                break
            except PermissionError:
                if attempt == 19:
                    raise
                sleep(.25)


def main():
    with test_profile() as profile:
        with ThreadingHTTPServer(('127.0.0.1', 0), Handler) as server:
            Thread(target=server.serve_forever, daemon=True).start()
            url = f'http://127.0.0.1:{server.server_port}/'
            try:
                for launch in range(2):
                    result = run(
                        [ROOT / 'target/debug/rust-browser.exe', '--second-monitor',
                         '--smoke', f'--profile={profile}', f'--url={url}'],
                        check=True, capture_output=True, text=True, timeout=25,
                    )
                    print(result.stdout, end='')
                    assert 'DISPLAY2' in result.stdout, result.stderr
                    assert any('fullscreen monitor:' in line and 'DISPLAY2' in line
                               for line in result.stdout.splitlines()), result.stderr
                    assert len(Handler.frames) == launch + 1, Handler.frames
                assert Handler.visits == [False, True], Handler.visits
                print(f'Persistent cookie survived restart. Local first frames: {Handler.frames} ms.')
            finally:
                server.shutdown()


if __name__ == '__main__':
    main()
