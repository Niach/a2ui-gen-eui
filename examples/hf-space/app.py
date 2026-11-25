"""Demo of generate_ui / refine_ui: an MCP server writes A2UI Express, the browser renders it as it arrives."""

import hmac
import json
import os
import re
import secrets
from collections.abc import AsyncIterator
from pathlib import Path
from urllib.parse import urlsplit

import httpx
import uvicorn
from a2ui_express import Catalogs, ExpressError
from fastapi import FastAPI, Request
from fastapi.responses import FileResponse, JSONResponse, StreamingResponse
from fastapi.staticfiles import StaticFiles

HERE = Path(__file__).parent
MAX_BODY = 200_000
LOOPBACK = {"127.0.0.1", "localhost", "::1"}
CSP = ("default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; "
       "img-src 'self' https: data: blob:; media-src 'self' https:; font-src 'self' data:; "
       "object-src 'none'; base-uri 'none'")

CATALOGS = Catalogs(json.loads((HERE / "vendor/core.schema.json").read_text()))


class ToolError(Exception):
    pass


def messages_of(parts: list[dict]) -> list[dict]:
    out = []
    for part in parts:
        if part.get("error"):
            raise ToolError(str(part["error"]))
        out += part.get("a2ui") or []
    return out


def compile_express(express: str) -> list[dict]:
    return messages_of(CATALOGS.parse_response(express, surface_id="main"))


def lines_of(express: str) -> str:
    body = express.strip().removeprefix("<a2ui>").removesuffix("</a2ui>")
    return "\n".join(line.strip() for line in body.splitlines() if line.strip())


class Screen:
    """Compiles statements as they arrive. They are not validated yet: an error only ends the preview."""

    def __init__(self) -> None:
        self.restart()

    def restart(self) -> None:
        self.statements: list[str] = []
        self.roots = self.surfaces = 0
        self.ok = True
        self.shown = False
        self.stream = CATALOGS.stream(surface_id="main")
        self.stream.push("<a2ui>\n")

    def push(self, statement: str) -> list[dict]:
        """The events for the browser."""
        surface = bool(re.match(r"surface\s*\(", statement))
        root = bool(re.match(r"root\s*=(?!=)", statement))
        events: list[dict] = []
        if (surface and self.surfaces) or (root and self.roots):
            # a new attempt; without `surface(…)` its data lines have already arrived, after the last component
            data: list[str] = []
            while not surface and self.statements and self.statements[-1].startswith("$"):
                data.insert(0, self.statements.pop())
            self.restart()
            events.append({"type": "reset"})
            for line in data:
                events += self._compile(line)
        self.surfaces += surface
        self.roots += root
        return events + [{"type": "statement", "source": statement}] + self._compile(statement)

    def _compile(self, statement: str) -> list[dict]:
        self.statements.append(statement)
        if not self.ok:
            return []
        try:
            messages = messages_of(self.stream.push(statement + "\n"))
        except (ExpressError, ToolError):
            self.ok = False
            return []
        self.shown |= bool(messages)
        return [{"type": "messages", "messages": messages}] if messages else []

    def final(self, express: str) -> list[dict]:
        """The final reply decides: render it again unless it is what is already on the screen."""
        if self.ok and self.shown and lines_of("\n".join(self.statements)) == lines_of(express):
            try:
                messages = messages_of(self.stream.push("</a2ui>")) + messages_of(self.stream.finish())
                return [{"type": "messages", "messages": messages}] if messages else []
            except (ExpressError, ToolError):
                pass
        return [{"type": "reset"}, {"type": "messages", "messages": compile_express(express)}]


async def sse(response: httpx.Response) -> AsyncIterator[dict]:
    data: list[str] = []
    async for line in response.aiter_lines():
        if line.startswith("data:"):
            data.append(line[5:].removeprefix(" "))
        elif not line and data:
            yield json.loads("\n".join(data))
            data = []
    if data:
        yield json.loads("\n".join(data))


class Mcp:
    """A client for one MCP server over streamable HTTP."""

    def __init__(self, url: str, token: str | None) -> None:
        self.url = url
        self.headers = {"Accept": "application/json, text/event-stream", "MCP-Protocol-Version": "2025-06-18"}
        if token:
            self.headers["X-Genui-Token"] = token

    async def call(self, tool: str, arguments: dict) -> AsyncIterator[dict | str]:
        """Yields statements (str) and, last, the result (dict)."""
        call_id, token = secrets.token_hex(8), secrets.token_hex(8)
        body = {"jsonrpc": "2.0", "id": call_id, "method": "tools/call",
                "params": {"name": tool, "arguments": arguments, "_meta": {"progressToken": token}}}
        # no environment proxies: the token goes to the configured server only
        async with (httpx.AsyncClient(timeout=httpx.Timeout(300, connect=5), trust_env=False) as client,
                    self._session(client) as session,
                    client.stream("POST", self.url, json=body, headers=self.headers | session) as response):
            if response.status_code != 200:
                raise ToolError("the server refused the token" if response.status_code in (401, 403)
                                else f"the server answered HTTP {response.status_code}")
            if response.headers.get("content-type", "").startswith("text/event-stream"):
                async for message in sse(response):
                    params = message.get("params") or {}
                    if message.get("method") == "notifications/progress" and params.get("progressToken") == token:
                        if isinstance(params.get("message"), str):
                            yield params["message"]
                    elif message.get("id") == call_id:
                        yield self._result(message)
                        return
                raise ToolError("the server ended the stream without a result")
            yield self._result(json.loads(await response.aread()))

    def _session(self, client: httpx.AsyncClient):
        return Session(client, self.url, self.headers)

    @staticmethod
    def _result(message: dict) -> dict:
        if error := message.get("error"):
            raise ToolError(error.get("message") or "the server reported an error")
        result = message.get("result") or {}
        content = result.get("content") or []
        text = "\n".join(c["text"] for c in content if c.get("type") == "text")
        if result.get("isError"):
            raise ToolError(text or "the tool failed")
        out = result.get("structuredContent") or {}
        return out | {"express": out.get("express") or text}


class Session:
    """The MCP handshake. Yields the session header, if the server uses one."""

    def __init__(self, client: httpx.AsyncClient, url: str, headers: dict) -> None:
        self.client, self.url, self.headers = client, url, headers

    async def __aenter__(self) -> dict:
        r = await self.client.post(self.url, headers=self.headers, json={
            "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "a2ui-gen-eui-demo", "version": "0.1.0"}}})
        if r.status_code in (401, 403):
            raise ToolError("the server refused the token")
        if r.status_code != 200:
            raise ToolError(f"the server answered HTTP {r.status_code}")
        sid = r.headers.get("mcp-session-id")
        self.extra = {"Mcp-Session-Id": sid} if sid else {}
        await self.client.post(self.url, headers=self.headers | self.extra,
                               json={"jsonrpc": "2.0", "method": "notifications/initialized"})
        return self.extra

    async def __aexit__(self, *_: object) -> None:
        if self.extra:
            try:
                await self.client.delete(self.url, headers=self.headers | self.extra)
            except httpx.HTTPError:
                pass


async def events(mcp: Mcp, tool: str, arguments: dict) -> AsyncIterator[dict]:
    screen, streamed = Screen(), False
    try:
        async for item in mcp.call(tool, arguments):
            if isinstance(item, str):
                streamed = True
                for event in screen.push(item):
                    yield event
            else:
                for event in screen.final(item["express"]):
                    yield event
                yield {"type": "done", "express": item["express"], "streamed": streamed,
                       "attempts": item.get("attempts"), "tokens": item.get("tokens")}
    except (ToolError, ExpressError) as e:
        yield {"type": "error", "message": str(e)}
    except (httpx.ConnectError, httpx.ConnectTimeout):
        yield {"type": "error", "message": "unavailable: the model server does not answer (GENUI_URL)"}
    except httpx.TimeoutException:
        yield {"type": "error", "message": "unavailable: the model server took too long"}
    except (httpx.HTTPError, ValueError, AttributeError, TypeError) as e:
        yield {"type": "error", "message": f"the model server sent no usable reply ({type(e).__name__})"}


def local_host(value: str, port: int) -> bool:
    try:
        parts = urlsplit("//" + value)
        return parts.hostname in LOOPBACK and (parts.port or 80) == port and "@" not in value
    except ValueError:
        return False


def create_app(url: str, token: str | None = None, key: str | None = None,
               host: str = "127.0.0.1", port: int = 7860) -> FastAPI:
    app = FastAPI(docs_url=None, redoc_url=None, openapi_url=None)
    mcp = Mcp(url, token)
    shell = compile_express((HERE / "static/shell.a2ui").read_text())
    loopback = host in LOOPBACK

    @app.middleware("http")
    async def guard(request: Request, call_next):
        asked, origin = request.headers.get("host", ""), request.headers.get("origin")
        foreign = loopback and not local_host(asked, port)
        if origin is not None:
            foreign |= urlsplit(origin).netloc.lower() != asked.lower()
        if foreign:
            return JSONResponse({"message": "foreign Host or Origin"}, 403)
        response = await call_next(request)
        response.headers["Content-Security-Policy"] = CSP + ("; frame-ancestors 'none'" if loopback else "")
        response.headers["Referrer-Policy"] = "no-referrer"
        response.headers["X-Content-Type-Options"] = "nosniff"
        response.headers.setdefault("Cache-Control", "no-cache")
        return response

    async def run(request: Request, tool: str, build) -> JSONResponse | StreamingResponse:
        def refuse(status: int, message: str) -> JSONResponse:
            return JSONResponse({"message": message}, status)

        if key and not hmac.compare_digest(request.headers.get("x-demo-key", "").encode(), key.encode()):
            return refuse(401, "wrong or missing key: open the page with its #key=… address")
        if request.headers.get("content-type", "").split(";")[0] != "application/json":
            return refuse(415, "JSON only")
        raw = b""
        async for chunk in request.stream():
            raw += chunk
            if len(raw) > MAX_BODY:
                return refuse(413, "the request is too long")
        try:
            arguments = build(json.loads(raw))
        except (ValueError, KeyError, TypeError, AttributeError):
            return refuse(400, "bad request")

        async def lines():
            async for event in events(mcp, tool, arguments):
                yield json.dumps(event, ensure_ascii=False) + "\n"

        return StreamingResponse(lines(), media_type="application/x-ndjson", headers={"Cache-Control": "no-store"})

    def text(body: dict, name: str) -> str:
        value = body[name].strip()
        if not value:
            raise ValueError(name)
        return value

    @app.post("/api/generate")
    async def generate(request: Request):
        def build(body: dict) -> dict:
            data = body.get("data")
            return {"request": text(body, "request")} | ({} if data is None else {"data": data})
        return await run(request, "generate_ui", build)

    @app.post("/api/refine")
    async def refine(request: Request):
        return await run(request, "refine_ui", lambda b: {"current": text(b, "current"), "change": text(b, "change")})

    @app.get("/shell.json")
    async def shell_json():
        return JSONResponse(shell)

    @app.get("/")
    async def index():
        return FileResponse(HERE / "static/index.html")

    app.mount("/static", StaticFiles(directory=HERE / "static"))
    app.mount("/vendor", StaticFiles(directory=HERE / "vendor/web"))
    app.mount("/assets", StaticFiles(directory=HERE / "vendor/web/assets"))  # the renderer's wasm
    return app


def main() -> None:
    host, port = os.environ.get("HOST", "127.0.0.1"), int(os.environ.get("PORT", "7860"))
    token = None
    if path := os.environ.get("GENUI_TOKEN_FILE"):
        token = Path(path).expanduser().read_text().strip() or None
        if not token:
            raise SystemExit("GENUI_TOKEN_FILE is empty")
    app = create_app(os.environ.get("GENUI_URL", "http://127.0.0.1:8765/mcp"), token,
                     os.environ.get("DEMO_KEY") or None, host, port)
    uvicorn.run(app, host=host, port=port, access_log=False, log_level="warning", server_header=False)


if __name__ == "__main__":
    main()
