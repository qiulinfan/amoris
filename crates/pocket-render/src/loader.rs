//! Where model assets come from. Natively, a worker thread imports glTF files under the project
//! root, so a large model never freezes a frame (aipocket's OBJ import froze for 14-27 s); in the
//! browser the page fetches them and hands the bytes in.

use pocket_assets::mesh::ModelAsset;

/// A source of model assets by project-relative path.
pub trait AssetSource {
    /// Starts loading `path` (called once per path).
    fn request(&mut self, path: &str);
    /// Loads that finished since the last call.
    fn poll(&mut self) -> Vec<(String, Result<ModelAsset, String>)>;
}

/// No assets: only primitives draw.
#[derive(Default)]
pub struct NoAssets;

impl AssetSource for NoAssets {
    fn request(&mut self, path: &str) {
        log::warn!("no asset source: {path} will not load");
    }
    fn poll(&mut self) -> Vec<(String, Result<ModelAsset, String>)> {
        Vec::new()
    }
}

/// Imports from the file system on a worker thread.
#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
pub struct FileAssets {
    tx: std::sync::mpsc::Sender<String>,
    rx: std::sync::mpsc::Receiver<(String, Result<ModelAsset, String>)>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
impl FileAssets {
    pub fn new(root: std::path::PathBuf) -> FileAssets {
        let (tx, jobs) = std::sync::mpsc::channel::<String>();
        let (done, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("pocket-assets".into())
            .spawn(move || {
                for path in jobs {
                    let full = root.join(&path);
                    let t = std::time::Instant::now();
                    let r = pocket_assets::import::import_gltf(&full).map_err(|p| p.message);
                    if let Ok(a) = &r {
                        log::info!(
                            "imported {path}: {} meshes, {} triangles, {} images in {:.0} ms",
                            a.meshes.len(),
                            a.triangles(),
                            a.images.len(),
                            t.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    if done.send((path, r)).is_err() {
                        break;
                    }
                }
            })
            .ok();
        FileAssets { tx, rx }
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
impl AssetSource for FileAssets {
    fn request(&mut self, path: &str) {
        let _ = self.tx.send(path.to_owned());
    }
    fn poll(&mut self) -> Vec<(String, Result<ModelAsset, String>)> {
        self.rx.try_iter().collect()
    }
}
