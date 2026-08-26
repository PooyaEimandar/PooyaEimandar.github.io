# Pooya Eimandar — Personal Website

Pooya's personal timeline rendered in real time with Rust, WebAssembly, WebGPU,
and the [`sib::render`](https://github.com/PooyaEimandar/sib) module.

## Build

Requirements:

- Rust with the `wasm32-unknown-unknown` target
- `wasm-bindgen-cli` 0.2.126
- Node.js and npm

```sh
npm ci
./build.sh --release
```

`data/timeline.json` is the timeline's single source of truth. The build embeds
it in the Rust/WebGPU renderer and generates both the semantic homepage copy
and the crawlable `/timeline/` page from the same data.

Production WebGPU requires HTTPS. The TypeScript bridge redirects the public
`pooya.ai` hosts to HTTPS, checks the secure context and `navigator.gpu`, then
lets Sib select the adapter that is compatible with the rendered surface.
