#!/usr/bin/env python3
"""Splits the fused Qwen3.5 tensors of a GGUF made with a llama.cpp newer than the one this crate pins.

    scripts/split-gguf.py IN.gguf OUT.gguf

November 2026's convert_hf_to_gguf.py writes `attn_qkv_gate` and `ssm_beta_alpha`, where older versions write two
tensors each. Both loaders read the separate form. The fused tensors are plain concatenations of rows, so the
split is exact, quantised or not. Standard library only.
"""

import struct
import sys

# ggml type -> (block size, bytes per block)
TYPES = {0: (1, 4), 1: (1, 2), 2: (32, 18), 3: (32, 20), 6: (32, 22), 7: (32, 24), 8: (32, 34), 10: (256, 84),
         11: (256, 110), 12: (256, 144), 13: (256, 176), 14: (256, 210), 30: (1, 2)}
SIZES = {0: 1, 1: 1, 2: 2, 3: 2, 4: 4, 5: 4, 6: 4, 7: 1, 10: 8, 11: 8, 12: 8}


def main(src: str, dst: str) -> None:
    f = open(src, "rb")

    def u32():
        return struct.unpack("<I", f.read(4))[0]

    def u64():
        return struct.unpack("<Q", f.read(8))[0]

    def value(t):
        if t == 8:
            return f.read(u64())
        if t == 9:
            et, n = u32(), u64()
            if et == 8:
                for _ in range(n):
                    f.read(u64())
            else:
                f.read(SIZES[et] * n)
            return None
        return int.from_bytes(f.read(SIZES[t]), "little")

    if f.read(4) != b"GGUF":
        sys.exit("not a GGUF file")
    version, n_tensors, n_kv = u32(), u64(), u64()
    start, kv = f.tell(), {}
    for _ in range(n_kv):
        key = f.read(u64())
        kv[key] = value(u32())
    kv_end = f.tell()
    infos = []
    for _ in range(n_tensors):
        name = f.read(u64()).decode()
        dims = [u64() for _ in range(u32())]
        infos.append((name, dims, u32(), u64()))
    align = kv.get(b"general.alignment") or 32
    pad = lambda n: (n + align - 1) // align * align  # noqa: E731
    base = pad(f.tell())
    by_name = {i[0]: i for i in infos}

    def size(dims, t):
        block, per = TYPES[t]
        rows = 1
        for d in dims[1:]:
            rows *= d
        assert dims[0] % block == 0
        return dims[0] // block * per * rows

    out = []  # name, dims, type, source offset
    split = 0
    for name, dims, t, off in infos:
        layer = name.rsplit(".", 2)[0]
        if name.endswith(".attn_qkv_gate.weight"):
            gate = by_name[f"{layer}.ssm_out.weight"][1][0]
            parts = [("attn_qkv", dims[1] - gate), ("attn_gate", gate)]
        elif name.endswith(".ssm_beta_alpha.weight"):
            parts = [("ssm_beta", dims[1] // 2), ("ssm_alpha", dims[1] // 2)]
        else:
            out.append((name, dims, t, off))
            continue
        split += 1
        for part, rows in parts:
            out.append((f"{layer}.{part}.weight", [dims[0], rows], t, off))
            off += size([dims[0], rows], t)
    if not split:
        sys.exit("no fused tensors, nothing to do")

    head = b"GGUF" + struct.pack("<IQQ", version, len(out), n_kv)
    f.seek(start)
    head += f.read(kv_end - start)
    at = 0
    for name, dims, t, _ in out:
        raw = name.encode()
        head += struct.pack("<Q", len(raw)) + raw + struct.pack(f"<I{len(dims)}QIQ", len(dims), *dims, t, at)
        at += pad(size(dims, t))
    with open(dst, "wb") as g:
        g.write(head + b"\0" * (pad(len(head)) - len(head)))
        for _, dims, t, off in out:
            n = size(dims, t)
            f.seek(base + off)
            data = f.read(n)
            if len(data) != n:
                sys.exit("the input is truncated")
            g.write(data + b"\0" * (pad(n) - n))
    print(f"{split} tensors split -> {dst}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
