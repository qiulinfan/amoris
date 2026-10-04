//! The renderer in the browser (charter 4.4, 4.5): `Viewport` draws the host's world on a canvas
//! with WebGPU, from the same render feed the native window uses (`RenderFrame`s over the host's
//! `/render` WebSocket). Model assets are fetched by the page and handed in as bytes. The page's
//! glue (`web/viewport.js`) owns the socket, the asset fetches and the animation frame loop.

#[cfg(target_arch = "wasm32")]
mod web;

use std::cell::RefCell;
use std::rc::Rc;

use pocket_assets::mesh::ModelAsset;
use pocket_render::AssetSource;

/// Asset requests queued for the page, and loads it completed.
#[derive(Clone, Default)]
pub struct PageAssets {
    pub requests: Rc<RefCell<Vec<String>>>,
    pub done: Rc<RefCell<Vec<(String, Result<ModelAsset, String>)>>>,
}

impl AssetSource for PageAssets {
    fn request(&mut self, path: &str) {
        self.requests.borrow_mut().push(path.to_owned());
    }

    fn poll(&mut self) -> Vec<(String, Result<ModelAsset, String>)> {
        std::mem::take(&mut *self.done.borrow_mut())
    }
}

impl PageAssets {
    /// Imports a fetched `.glb` (or reports the fetch's failure).
    pub fn deliver(&self, path: &str, bytes: Result<&[u8], String>) {
        let r = bytes.and_then(|b| {
            pocket_assets::import::import_glb_bytes(b).map_err(|p| p.message.clone())
        });
        self.done.borrow_mut().push((path.to_owned(), r));
    }
}
