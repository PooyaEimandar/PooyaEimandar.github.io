# Pooya Eimandar | [Personal Website](https://pooya.ai)

Pooya's personal timeline rendered in real time with Rust, WebAssembly, WebGPU,
and the [`sib::render`](https://github.com/PooyaEimandar/sib) module.

## Build

Requirements:

- Rust with the `wasm32-unknown-unknown` target
- `wasm-bindgen-cli` 0.2.126
- Node.js 22.18 or newer and npm

```sh
npm ci
./build.sh --release
```

`data/timeline.json` is the timeline's single source of truth. The build embeds
it in the Rust/WebGPU renderer and generates both the semantic homepage copy
and the crawlable `/timeline/` page from the same data.

The maintained browser and build-tool sources are TypeScript: `app.ts` and
`scripts/build-static-timeline.ts`. The build generates JavaScript only inside
the ignored `_site/` output directory: compiled browser code and the
`wasm-bindgen` loader required to connect Rust/WebAssembly to the browser.
These generated files are not committed or edited by hand.

Run `npm run check` to type-check the browser code and build scripts without
generating JavaScript.

## Local preview

Serve the built `_site/` directory, not the repository root:

```sh
python3 -m http.server 8090 --bind 127.0.0.1 --directory _site
```

Open [the local website](http://127.0.0.1:8090/).

## Deployment

GitHub Actions runs the release build on every push to `master`, generating and
validating both HTML timelines on the runner before deploying `_site/`.
