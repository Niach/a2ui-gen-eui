"""Fetches the renderer, the catalog and the icons into vendor/. Every download is pinned by hash."""

import base64
import hashlib
import io
import json
import tarfile
import urllib.request
from pathlib import Path

OUT = Path(__file__).parent / "vendor"
NPM = "https://registry.npmjs.org/"
EUI = (NPM + "@exponential_app/eui/-/eui-0.22.0.tgz", "sha512",
       "TODO: dist.integrity of the published 0.22.0")
ELEMENT = (NPM + "@exponential_app/eui-element/-/eui-element-0.22.0.tgz", "sha512",
           "TODO: dist.integrity of the published 0.22.0")
LUCIDE = (NPM + "lucide-static/-/lucide-static-0.544.0.tgz", "sha512",
          "2PkMTYcjQkIzYbd5u2g7UtRaNDP7dLQT107MmYBdvnZZCO3Vj0nqH6HA5AkUS2Era8D1ITb/UxrOkfdH/qN2Ag==")
# the catalog's semantic icon names (nav-home, ui-add, …) -> lucide
ICONS = ("https://raw.githubusercontent.com/Niach/exponential/30abe355fdce83247722d97e23b46a128c12f25c"
         "/packages/icons/icons.json", "sha256", "bz7/64ebiewh8ewyD8XAYz2dunl+HOVTiL/R58oM0io=")


def fetch(url: str, algorithm: str, digest: str) -> bytes:
    with urllib.request.urlopen(url, timeout=60) as r:
        body = r.read()
    if base64.b64encode(hashlib.new(algorithm, body).digest()).decode() != digest:
        raise SystemExit(f"{url}: wrong {algorithm}")
    return body


def files(package: tuple[str, str, str], keep) -> dict[str, bytes]:
    with tarfile.open(fileobj=io.BytesIO(fetch(*package))) as tar:
        return {m.name.removeprefix("package/"): tar.extractfile(m).read()
                for m in tar if m.isfile() and keep(m.name.removeprefix("package/"))}


def put(name: str, body: bytes) -> None:
    path = OUT / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(body)


def main() -> None:
    eui = files(EUI, lambda n: n in ("catalog/core.schema.json", "LICENSE", "NOTICE"))
    put("core.schema.json", eui["catalog/core.schema.json"])
    element = files(ELEMENT, lambda n: n in ("dist/standalone.js", "LICENSE", "NOTICE") or n.startswith("dist/assets/"))
    for name, body in element.items():
        put("web/" + name.removeprefix("dist/"), body)
    lucide = files(LUCIDE, lambda n: n.startswith("icons/") and n.endswith(".svg") or n == "LICENSE")
    put("web/LICENSE.lucide", lucide["LICENSE"])

    names = json.loads(eui["catalog/core.schema.json"])["components"]["Badge"]["properties"]["icon"]["enum"]
    registry = json.loads(fetch(*ICONS))
    icons = {}
    for name in names:
        svg = lucide.get(f"icons/{registry['semantic'].get(name, name)}.svg")
        if svg:
            icons[name] = svg.decode().split("-->", 1)[-1].strip()
    put("web/icons.json", json.dumps(icons).encode())
    print(f"vendor/: renderer, catalog, {len(icons)} of {len(names)} icons (the rest use the renderer's own)")


if __name__ == "__main__":
    main()
