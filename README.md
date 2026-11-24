# a2ui-gen-eui

Generates user interfaces as [A2UI Express](https://a2ui.org/concepts/inference-formats/) with a small local
model. Give it a request (and data), get a validated screen for the
[eui](https://www.npmjs.com/package/@exponential_app/eui) component catalog, streamed while it is written.

- Local inference with llama.cpp. No Python, no network calls.
- Grammar-guided decoding: the grammar is generated from the catalog's JSON schema, so a reply only names
  components, properties, enum values and functions the catalog has.
- Every reply is compiled with [`a2ui-express`](https://crates.io/crates/a2ui-express). What a grammar cannot
  know (ids) is repaired by dropping unreachable statements, else the reply is sampled again (at most twice).
- Streaming: statements and ready-to-render A2UI messages as they complete.
- An MCP server with `generate_ui` and `refine_ui`.

## Quick start

Needs Rust, CMake, a C++ compiler and libclang (for bindgen).

```sh
# a model: the base Qwen3.5 0.8B is enough to try the runtime, but it was not trained on Express
cargo run --release -p a2ui-gen-eui-mcp --features download -- download --dir models

cargo run --release -p a2ui-gen-eui --example generate -- models/MODEL.gguf "A login screen"
```

If bindgen does not find `stdbool.h` (libclang without clang's headers):
`export BINDGEN_EXTRA_CLANG_ARGS="-I$(gcc -print-file-name=include)"`.

## API

```toml
a2ui-gen-eui = { git = "https://github.com/Niach/a2ui-gen-eui" }
```

```rust
use a2ui_gen_eui::{ExpressBlock, Generator};

let generator = Generator::load("model.gguf")?;             // load_with: threads, context, GPU layers, sha256
let stream = generator.generate(request, Some(&data), None)?; // None = eui's catalog

for block in stream.blocking() {                             // or poll it: Generation is a futures Stream
    match block? {
        ExpressBlock::Statement { kind, source } => {}       // `root = Page(…)`, one line of Express
        ExpressBlock::Messages(messages) => {}               // A2UI JSON for the renderer
        ExpressBlock::Restart { attempt, reason } => {}      // invalid: delete the surface, a new attempt follows
        ExpressBlock::Complete(ui) => {}                     // ui.express, ui.messages, ui.attempts, ui.stats
    }
}
let ui = generator.refine(&ui.express, "Add a notes field", None)?.wait()?;
```

`root` is the first component, but models may write their data lines (`$/path = value`) before it. The first
`Messages` come with `root` and include that data. Dropping a `Generation` cancels it. Generations of one
`Generator` run one after the other. The defaults (temperature 0.2, top-p 0.8, 4096 tokens, 2 retries at 0.7)
are the ones the models were tuned for; `generate_with` / `refine_with` change them.

**Custom catalogs.** `Catalog::from_json(schema)` takes any A2UI v1.0 catalog: grammar and validation follow it,
and its signatures go into the system prompt. The default models know eui's catalog by heart and were not
trained on others, so expect valid but less fitting screens.

Without the default feature `llama` the crate has no native code: prompts, grammars (`grammar::gbnf` for
llama.cpp and vLLM, `grammar::lark` for llguidance), streaming and validation.

## Why llama.cpp

The models are Qwen3.5 (hybrid Gated DeltaNet / attention). Checked in November 2026:

| | Qwen3.5 GGUF | CPU | Metal | CUDA | Vulkan | iOS / Android |
|---|---|---|---|---|---|---|
| llama.cpp via `llama-cpp-2` 0.1.161 | yes | yes | yes | yes | yes | build targets of the binding, built in CI |
| `mistralrs` 0.8.1 | no (Qwen3.5 from safetensors only) | yes | yes | yes | no | no |
| `candle-transformers` 0.11.1 | no Qwen3.5 | yes | yes | yes | no | possible |

Neither alternative loads the artefact, so there was no speed to compare. The MCP server with llama.cpp, all CPU
kernels, the grammar engine and the eui catalog is a **10.2 MB** binary (linux x86_64, stripped, needs only libc
and libstdc++). The binding follows llama.cpp closely and is pinned to an exact version.

The grammar runs in [llguidance](https://github.com/guidance-ai/llguidance) (Rust), not in llama.cpp's GBNF
sampler, because eui's grammar has thousands of alternatives. Qwen3.5 0.8B Q4_K_M, CPU only, i9-13900K, 8
threads, machine in use by other jobs (load 12 to 20):

| decoding | tokens/s | grammar + sampling per token |
|---|---|---|
| no grammar | 38.3 | 0.44 ms |
| llguidance | 33.7 | 0.47 ms |
| llama.cpp GBNF | 15.0 | 35.3 ms |

llguidance's overhead against free decoding is small and within the noise of that machine; GBNF halves the speed.
The first request of a catalog also compiles its grammar, once. `cargo run --release -p a2ui-gen-eui --example
bench -- MODEL.gguf 8` repeats this.

## Platforms

| | back end | state |
|---|---|---|
| linux x86_64 | CPU (AVX2) | built, tested, measured with a model |
| macOS arm64 | Metal | built and tested in CI, no model run |
| Windows x86_64 (MSVC) | CPU | built and tested in CI, no model run |
| iOS arm64 | Metal (`--features metal`) | library builds in CI |
| Android arm64 | CPU | library builds in CI (NDK, API 28) |
| linux | Vulkan (`--features vulkan`) | type-checked and native code built in CI |
| linux, Windows | CUDA (`--features cuda`) | feature only, not built yet |

`--features openmp` replaces ggml's thread pool with OpenMP. x86_64 builds need AVX2. To build for the machine at
hand, set `RUSTFLAGS="-C target-cpu=native"`.

## Models

A GGUF file from a path. `LoadOptions::sha256` (`--sha256`) refuses any other file. The feature `download` adds
`model::download(&ModelSpec::new(url, sha256), dir, progress)`: an explicit call that fetches a file once and
puts it in place only after its hash matched. Inference never touches the network.

The prompt contract (`prompt`): a fixed system sentence, the request, optionally the data as JSON; for a change
the current screen and the instruction; Qwen's chat template with an empty think block. Text of the caller's is
tokenized as plain text, so control tokens written in it have no effect.

## MCP

```sh
a2ui-gen-eui-mcp --model model.gguf [--sha256 HEX] [--threads N] [--context N] [--gpu-layers N]
a2ui-gen-eui-mcp --model model.gguf --http 127.0.0.1:8765     # streamable HTTP on /mcp
```

```json
{ "mcpServers": { "genui": { "command": "a2ui-gen-eui-mcp", "args": ["--model", "/path/model.gguf"] } } }
```

- `generate_ui {request, data?, catalog?}` and `refine_ui {current, change, catalog?}` return the validated
  `<a2ui>` block as text and `{express, attempts, pruned}` as structured content.
- With a progress token, stdio clients get every statement as a progress notification.
- HTTP answers each POST with one JSON reply. It binds to loopback only and refuses foreign `Origin` and `Host`
  headers; it has no authentication.

## Development

```sh
cargo test --workspace --no-default-features   # no native build, no model
cargo test --workspace
scripts/sync-eui.sh 0.21.0                     # re-vendor the catalog
```

## Licence

Apache-2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE). The eui catalog under `crates/a2ui-gen-eui/vendor/eui`
keeps its own licence and notice (Apache-2.0). Model weights have their own licences.
