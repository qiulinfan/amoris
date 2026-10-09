//! Where model assets come from. Natively, a worker thread imports glTF files under the project
//! root, so a large model never freezes a frame (Amoris Pioneer's OBJ import froze for 14-27 s); in the
//! browser the page fetches them and hands the bytes in.

use pocket_assets::gi::{BakedGi, NeuralGi};
use pocket_assets::mesh::ModelAsset;
use pocket_assets::neural::NeuralTexture;

/// Whether a material path names a neural texture (`materials/brick.ntex`). Every look's material
/// passes through here, and material names are any UTF-8 a script or a glTF file sets (`金属`,
/// `models/x.glb#屋顶`): the suffix is compared as bytes, never by slicing the string at a byte
/// offset that may fall inside a character.
pub fn is_neural_texture(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() > 5 && b[b.len() - 5..].eq_ignore_ascii_case(b".ntex")
}

/// A source of model assets by project-relative path.
pub trait AssetSource {
    /// Starts loading `path` (called once per path).
    fn request(&mut self, path: &str);
    /// Loads that finished since the last call.
    fn poll(&mut self) -> Vec<(String, Result<ModelAsset, String>)>;
    fn request_baked_gi(&mut self, path: &str) {
        log::warn!("this asset source cannot load baked GI: {path}");
    }
    fn poll_baked_gi(&mut self) -> Vec<(String, Result<BakedGi, String>)> {
        Vec::new()
    }
    fn request_neural_gi(&mut self, path: &str) {
        log::warn!("this asset source cannot load neural GI: {path}");
    }
    fn poll_neural_gi(&mut self) -> Vec<(String, Result<NeuralGi, String>)> {
        Vec::new()
    }
    /// Starts loading a neural texture material (`.ntex`, docs/spec/neural-textures.md).
    fn request_neural_texture(&mut self, path: &str) {
        log::warn!("this asset source cannot load neural textures: {path}");
    }
    fn poll_neural_textures(&mut self) -> Vec<(String, Result<NeuralTexture, String>)> {
        Vec::new()
    }
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
    tx: std::sync::mpsc::Sender<Job>,
    rx: std::sync::mpsc::Receiver<Loaded>,
    models_done: Vec<(String, Result<ModelAsset, String>)>,
    gi_done: Vec<(String, Result<BakedGi, String>)>,
    neural_done: Vec<(String, Result<NeuralGi, String>)>,
    textures_done: Vec<(String, Result<NeuralTexture, String>)>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
enum Job {
    Model(String),
    Gi(String),
    Neural(String),
    Texture(String),
}
#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
enum Loaded {
    Model(String, Result<ModelAsset, String>),
    Gi(String, Result<BakedGi, String>),
    Neural(String, Result<NeuralGi, String>),
    Texture(String, Result<NeuralTexture, String>),
}

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
impl FileAssets {
    pub fn new(root: std::path::PathBuf) -> FileAssets {
        let (tx, jobs) = std::sync::mpsc::channel::<Job>();
        let (done, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("pocket-assets".into())
            .spawn(move || {
                for job in jobs {
                    let path = match job {
                        Job::Texture(path) => {
                            let result = NeuralTexture::load(&root.join(&path));
                            if done.send(Loaded::Texture(path, result)).is_err() {
                                break;
                            }
                            continue;
                        }
                        Job::Neural(path) => {
                            let result = NeuralGi::load(&root.join(&path));
                            if done.send(Loaded::Neural(path, result)).is_err() {
                                break;
                            }
                            continue;
                        }
                        Job::Gi(path) => {
                            let result = BakedGi::load(&root.join(&path));
                            if done.send(Loaded::Gi(path, result)).is_err() {
                                break;
                            }
                            continue;
                        }
                        Job::Model(path) => path,
                    };
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
                    if done.send(Loaded::Model(path, r)).is_err() {
                        break;
                    }
                }
            })
            .ok();
        FileAssets {
            tx,
            rx,
            models_done: Vec::new(),
            gi_done: Vec::new(),
            neural_done: Vec::new(),
            textures_done: Vec::new(),
        }
    }

    fn collect(&mut self) {
        for item in self.rx.try_iter() {
            match item {
                Loaded::Model(path, result) => self.models_done.push((path, result)),
                Loaded::Gi(path, result) => self.gi_done.push((path, result)),
                Loaded::Neural(path, result) => self.neural_done.push((path, result)),
                Loaded::Texture(path, result) => self.textures_done.push((path, result)),
            }
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
impl AssetSource for FileAssets {
    fn request(&mut self, path: &str) {
        let _ = self.tx.send(Job::Model(path.to_owned()));
    }
    fn poll(&mut self) -> Vec<(String, Result<ModelAsset, String>)> {
        self.collect();
        std::mem::take(&mut self.models_done)
    }
    fn request_baked_gi(&mut self, path: &str) {
        let _ = self.tx.send(Job::Gi(path.to_owned()));
    }
    fn poll_baked_gi(&mut self) -> Vec<(String, Result<BakedGi, String>)> {
        self.collect();
        std::mem::take(&mut self.gi_done)
    }
    fn request_neural_gi(&mut self, path: &str) {
        let _ = self.tx.send(Job::Neural(path.to_owned()));
    }
    fn poll_neural_gi(&mut self) -> Vec<(String, Result<NeuralGi, String>)> {
        self.collect();
        std::mem::take(&mut self.neural_done)
    }
    fn request_neural_texture(&mut self, path: &str) {
        let _ = self.tx.send(Job::Texture(path.to_owned()));
    }
    fn poll_neural_textures(&mut self) -> Vec<(String, Result<NeuralTexture, String>)> {
        self.collect();
        std::mem::take(&mut self.textures_done)
    }
}

#[cfg(test)]
mod tests {
    use super::is_neural_texture;

    #[test]
    fn neural_texture_paths_are_recognized_in_any_script() {
        for yes in [
            "materials/brick.ntex",
            "materials/BRICK.NTEX",
            "材质/砖.ntex",
            "a.Ntex",
        ] {
            assert!(is_neural_texture(yes), "{yes}");
        }
        // Material names whose fifth byte from the end falls inside a character must not panic.
        for no in [
            "",
            ".ntex",
            "金属",
            "materials/红砖",
            "models/x.glb#屋顶",
            "砖ntex",
            "materials/brick.png",
            "materials/brick.ntex.png",
        ] {
            assert!(!is_neural_texture(no), "{no}");
        }
    }
}
