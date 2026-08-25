# Pooya Eimandar — Rust/WebGPU portfolio

Pooya's personal timeline rendered in real time with Rust, WebAssembly, WebGPU,
and the [`sib::render`](https://github.com/PooyaEimandar/sib) module.

## Build

Requirements:

- Rust with the `wasm32-unknown-unknown` target
- `wasm-bindgen-cli` 0.2.126
- Node.js and npm

```sh
npm ci
npm run check
npm run build
scripts/build-wasm.sh --release
```
