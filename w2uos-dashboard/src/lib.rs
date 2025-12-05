#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod panels;

#[cfg(target_arch = "wasm32")]
pub use app::*;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn run() {
    console_error_panic_hook::set_once();
    yew::Renderer::<app::App>::new().render();
}

#[cfg(not(target_arch = "wasm32"))]
/// Non-wasm builds are stubbed to keep workspace builds green.
pub fn run() {
    // Dashboard is only available for wasm targets.
}
