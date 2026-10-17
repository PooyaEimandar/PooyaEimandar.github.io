# Pooya Eimandar | [Personal Website](https://pooya.ai)

Pooya's personal timeline rendered in real time with Rust, WebAssembly, WebGPU,
and the [`sib::render`](https://github.com/PooyaEimandar/sib) module.

## Build

Requirements:

- Rust with the `wasm32-unknown-unknown` target
- `wasm-bindgen-cli` 0.2.129
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

## Browser resume regression check

After building, copy the local-only test page into the preview output:

```sh
cp tests/renderer-resume.html _site/renderer-resume.html
```

Open `/renderer-resume.html` on the local preview and run its three restore
cycles. It sends persisted `pagehide`/`pageshow` events to the real WebGPU
renderer and checks that each restore replaces the canvas, reconnects input,
and preserves the current slide. This simulates lifecycle delivery; also check
Home → Timeline → browser Back on a desktop and a mobile browser to exercise
their actual back/forward cache behavior. The test page is not deployed by the
build script.

## Deployment

GitHub Actions runs the release build on every push to `master`, generating and
validating both HTML timelines on the runner before deploying `_site/`.
