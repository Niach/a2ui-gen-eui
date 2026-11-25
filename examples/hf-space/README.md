---
title: a2ui-gen-eui
emoji: 🧩
colorFrom: gray
colorTo: indigo
sdk: docker
app_port: 7860
pinned: false
license: apache-2.0
---

# a2ui-gen-eui demo

Describe a screen, get a live one: the request goes to an MCP server with `generate_ui` / `refine_ui`, the
[A2UI Express](https://a2ui.org/concepts/inference-formats/) it answers is compiled here
([a2ui-express](https://crates.io/crates/a2ui-express)) and painted in the browser by
[`@exponential_app/eui-element`](https://www.npmjs.com/package/@exponential_app/eui-element). The controls are
an eui screen too (`static/shell.a2ui`).

The app has no model. Point `GENUI_URL` at a server, for example this repository's:

```sh
a2ui-gen-eui-mcp --model model.gguf --http 127.0.0.1:8765
```

## Streaming

The app asks for progress (`progressToken`, `Accept: text/event-stream`). A server that answers with an event
stream, one Express statement per `notifications/progress`, is rendered block by block; a second
`surface("main")` or `root` restarts the screen, and the final reply decides. **A server that answers with one
JSON reply delivers everything at once**: the first block then takes as long as the whole reply, and the app says
so. `a2ui-gen-eui-mcp --http` is such a server today. "First block" is the first rendered message; data lines
before `root` draw nothing.

Anything the renderer rejects is listed in the page once a screen is finished.

## Run

Python 3.11+ and a Rust toolchain (`a2ui-express` is built from source).

```sh
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
.venv/bin/python vendor.py      # renderer, catalog and icons from npm, pinned by hash
GENUI_URL=http://127.0.0.1:8765/mcp .venv/bin/python app.py    # http://127.0.0.1:7860
```

As a Space: this directory is the Space's root (`Dockerfile`, `app_port`). Set `GENUI_URL` in its settings, and
`DEMO_KEY` as a secret unless everybody may use your server.

| variable | default | |
|---|---|---|
| `GENUI_URL` | `http://127.0.0.1:8765/mcp` | MCP server, streamable HTTP |
| `GENUI_TOKEN_FILE` | none | file with a token, sent as `X-Genui-Token` |
| `DEMO_KEY` | none | if set, calls need this key: open `/#key=…` |
| `PORT` | `7860` | |
| `HOST` | `127.0.0.1` | `0.0.0.0` in the container only |

On a loopback `HOST`, requests with a foreign `Host` or `Origin` are refused (DNS rebinding, other web pages).

## Test

```sh
.venv/bin/pip install pytest && .venv/bin/python -m pytest tests     # needs vendor.py; no model, a fake server
```
