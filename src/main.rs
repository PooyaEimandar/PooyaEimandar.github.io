#[cfg(not(target_arch = "wasm32"))]
fn main() -> sib::render::RenderResult<()> {
    pooya_portfolio::run_portfolio()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
