import argparse
import json
import os
import re
import tempfile
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote

APPS = Path(__file__).resolve().parent / 'apps'

class Handler(SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def end_headers(self):
        self.send_header('Cache-Control', 'no-store')
        super().end_headers()

    def reply(self, value, status=200, content_type='application/json'):
        data = value.encode() if isinstance(value, str) else json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def task_path(self):
        task = unquote(self.path.removeprefix('/api/state/'))
        if not re.fullmatch('[a-z0-9-]+', task):
            self.reply({}, 400)
            return None
        return self.server.state_dir / (task + '.json')

    def do_GET(self):
        if self.path == '/healthz':
            return self.reply('ok', content_type='text/plain')
        if self.path.startswith('/api/state/'):
            path = self.task_path()
            if path is not None:
                self.reply(json.loads(path.read_text()) if path.exists() else {})
            return
        if self.path.startswith('/apps/'):
            return super().do_GET()
        self.reply({}, 404)

    def translate_path(self, path):
        relative = unquote(path.split('?', 1)[0]).removeprefix('/apps/')
        target = (APPS / relative).resolve()
        return str(target if target.is_relative_to(APPS) else APPS / '__invalid__')

    def do_POST(self):
        if self.path == '/api/reset':
            for path in self.server.state_dir.glob('*.json'):
                path.unlink(missing_ok=True)
            return self.reply({'ok': True})
        if not self.path.startswith('/api/state/'):
            return self.reply({}, 404)
        path = self.task_path()
        if path is None:
            return
        try:
            value = json.loads(self.rfile.read(int(self.headers.get('Content-Length', '0'))))
            if not isinstance(value, dict):
                raise ValueError()
        except (ValueError, TypeError):
            return self.reply({}, 400)
        fd, temporary = tempfile.mkstemp(dir=self.server.state_dir)
        with os.fdopen(fd, 'w') as output:
            json.dump(value, output)
        os.replace(temporary, path)
        self.reply({'ok': True})

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--host', default='127.0.0.1', choices=['127.0.0.1', 'localhost', '::1'])
    parser.add_argument('--port', type=int, default=8765)
    parser.add_argument('--state-dir', required=True, type=Path)
    args = parser.parse_args()
    args.state_dir.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    server.state_dir = args.state_dir
    server.serve_forever()
