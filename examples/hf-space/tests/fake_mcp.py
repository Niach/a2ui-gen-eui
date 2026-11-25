"""A tiny MCP server: an event stream (one statement per event) or one JSON reply.

    python tests/fake_mcp.py 8765 [json] [form] [broken]
"""

import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SCREEN = ['surface("main")', '$/name = "Ada"', "root = Page(title=\"Profile\", children=[card])",
          'card = Card(title="Person", children=[name])', "name = Text(text=$/name)"]
FORM = ['surface("main")', '$/form = {name: "", email: ""}', 'root = Page(title="Sign up", children=[form])',
        'form = Form(name="f", children=[nameField, emailField, send])',
        'nameField = Input(label="Name", name="name", value=$/form/name, checks=[?required("Required")])',
        'emailField = Input(label="Email", name="email", type="email", value=$/form/email, checks=[?required, ?email])',
        'send = FormActions(submit="Subscribe")']
# the renderer rejects it: `gone` is never defined
BROKEN = ['surface("main")', 'root = Page(title="Broken", children=[a, gone])', 'a = Text(text="here")']
BAD = ['surface("main")', '$/name = "old"', 'root = Page(title="Old", children=[x])', "x = Nope(a=1)"]


def express(statements: list[str]) -> str:
    return "<a2ui>\n" + "\n".join(statements) + "\n</a2ui>"


def make(stream: bool = True, delay: float = 0, attempts: list[list[str]] | None = None, token: str | None = None,
         final: str | None = None):
    attempts = attempts or [SCREEN]

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            if token and self.headers.get("X-Genui-Token") != token:
                return self.reply(401, {})
            m = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if "id" not in m:
                return self.reply(202, None)
            if m["method"] == "initialize":
                return self.reply(200, {"jsonrpc": "2.0", "id": m["id"], "result": {"capabilities": {}}})
            params = m["params"]
            said = json.dumps(params["arguments"])
            if "game" in said:
                result = {"isError": True, "content": [{"type": "text", "text": "unavailable: a game is running"}]}
            else:
                text = final or express(attempts[-1])
                result = {"content": [{"type": "text", "text": text}],
                          "structuredContent": {"express": text, "attempts": len(attempts)}}
            reply = {"jsonrpc": "2.0", "id": m["id"], "result": result}
            progress = params.get("_meta", {}).get("progressToken")
            if not (stream and progress and "text/event-stream" in self.headers.get("Accept", "")):
                return self.reply(200, reply)
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            n = 0
            for statement in [] if result.get("isError") else [s for a in attempts for s in a]:
                n += 1
                self.event({"jsonrpc": "2.0", "method": "notifications/progress",
                            "params": {"progressToken": progress, "progress": n, "message": statement}})
                time.sleep(delay)
            self.event(reply)

        def event(self, m):
            self.wfile.write(f"event: message\ndata: {json.dumps(m)}\n\n".encode())
            self.wfile.flush()

        def reply(self, code, body):
            raw = b"" if body is None else json.dumps(body).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def log_message(self, *_):
            pass

    return Handler


if __name__ == "__main__":
    SCREENS = [FORM] if "form" in sys.argv else [BROKEN] if "broken" in sys.argv else [BAD, SCREEN]
    ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])),
                        make(stream="json" not in sys.argv, delay=0.5, attempts=SCREENS)).serve_forever()
