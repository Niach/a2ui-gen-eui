import json
import sys
import threading
from contextlib import contextmanager
from http.server import ThreadingHTTPServer
from pathlib import Path

import httpx
import pytest
from fastapi.testclient import TestClient

sys.path[:0] = [str(Path(__file__).parents[1]), str(Path(__file__).parent)]
from app import create_app, local_host  # noqa: E402
from fake_mcp import BAD, SCREEN, express, make  # noqa: E402


@contextmanager
def demo(fake: dict | None = None, **options):
    server = ThreadingHTTPServer(("127.0.0.1", 0), make(**(fake or {})))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    app = create_app(f"http://127.0.0.1:{server.server_port}/mcp", **options)
    try:
        yield TestClient(app, base_url="http://127.0.0.1:7860")
    finally:
        server.shutdown()
        server.server_close()


def generate(client, request="a profile", **headers):
    r = client.post("/api/generate", json={"request": request}, headers=headers)
    assert r.status_code == 200, r.text
    return [json.loads(line) for line in r.text.splitlines()]


def types(events):
    return [e["type"] for e in events]


def test_streams_blocks():
    with demo() as c:
        ev = generate(c)
    assert "reset" not in types(ev)
    first = next(e for e in ev if e["type"] == "messages")
    assert first["messages"][0]["createSurface"]["dataModel"] == {"name": "Ada"}
    assert types(ev).count("messages") == 3
    assert ev[-1]["streamed"] and ev[-1]["express"] == express(SCREEN)


def test_restart():
    with demo({"attempts": [BAD, SCREEN]}) as c:
        ev = generate(c)
    assert types(ev).count("reset") == 1
    after = ev[types(ev).index("reset"):]
    assert [e["source"] for e in after if e["type"] == "statement"] == SCREEN


def test_restart_without_surface_keeps_the_new_data():
    old, new = BAD[1:], SCREEN[1:]
    with demo({"attempts": [old, new]}) as c:
        ev = generate(c)
    after = ev[types(ev).index("reset"):]
    first = next(e for e in after if e["type"] == "messages")
    assert first["messages"][0]["createSurface"]["dataModel"] == {"name": "Ada"}
    assert types(ev).count("reset") == 1


def test_the_final_reply_decides():
    with demo({"final": express(SCREEN[:3] + ['card = Text(text="final")'])}) as c:
        ev = generate(c)
    assert types(ev)[-3:] == ["reset", "messages", "done"]


def test_json_reply():
    with demo({"stream": False}) as c:
        ev = generate(c)
    assert types(ev) == ["reset", "messages", "done"] and ev[-1]["streamed"] is False


@pytest.mark.parametrize("stream", [True, False])
def test_tool_error(stream):
    with demo({"stream": stream}) as c:
        assert generate(c, "a game")[-1] == {"type": "error", "message": "unavailable: a game is running"}


def test_server_down():
    c = TestClient(create_app("http://127.0.0.1:9/mcp"), base_url="http://127.0.0.1:7860")
    assert generate(c)[-1]["message"].startswith("unavailable")


def test_token():
    with demo({"token": "t"}, token="t") as c:
        assert generate(c)[-1]["type"] == "done"
    with demo({"token": "t"}) as c:
        assert "token" in generate(c)[-1]["message"]


def test_key():
    with demo(key="k") as c:
        assert c.post("/api/generate", json={"request": "x"}).status_code == 401
        assert generate(c, **{"x-demo-key": "k"})[-1]["type"] == "done"


def test_guard():
    with demo() as c:
        assert c.get("/").status_code == 200
        assert c.get("/", headers={"host": "evil.example:7860"}).status_code == 403
        assert c.get("/", headers={"origin": "https://evil.example"}).status_code == 403
        assert c.post("/api/generate", content='{"request":"x"}', headers={"content-type": "text/plain"}).status_code == 415
        assert c.post("/api/generate", json={"request": " "}).status_code == 400
        assert c.post("/api/generate", json={"request": "x" * 300_000}).status_code == 413
    with demo(host="0.0.0.0") as c:
        assert c.get("/", headers={"host": "me.hf.space", "origin": "https://me.hf.space"}).status_code == 200
        assert c.get("/", headers={"host": "me.hf.space", "origin": "https://evil.example"}).status_code == 403
    assert local_host("localhost:7860", 7860) and not local_host("127.0.0.1:1", 7860)


def test_refine():
    with demo() as c:
        r = c.post("/api/refine", json={"current": express(SCREEN), "change": "more"})
        assert httpx.codes.is_success(r.status_code) and '"done"' in r.text
