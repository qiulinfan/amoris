//! The running engine's version (docs/spec/versions.md 3.1): what the top crate's build script
//! computed (`crates/pocket-app/build/engine_version.rs`, shared by `pocket-web`), installed once
//! where the binary starts with [`install_engine_version!`](crate::install_engine_version), before
//! anything records. Replays then carry the engine's `source`, `Verify` refuses a replay of another
//! source, and a recorded bundle's compiled JavaScript is reused only from the same source
//! (replay.md 2.4).

use pocket_persist::EngineVersion;
use pocket_sim::ContentHash;

/// The source hash's definition, over `(path, content)` pairs.
pub use pocket_persist::version::source_hash;
/// The C compiler that built QuickJS-ng (P7), `EngineVersion.c_compiler`.
pub use pocket_script::QJS_CC;

/// What a build script computed, as `install_engine_version!` passes it.
#[derive(Clone, Copy, Debug)]
pub struct BuiltEngine<'a> {
    pub semver: &'a str,
    pub commit: &'a str,
    /// 64 hex digits.
    pub source: &'a str,
    pub target: &'a str,
    pub profile: &'a str,
    pub contract: &'a str,
}

impl BuiltEngine<'_> {
    /// The version this build reports, with the C compiler the vendored QuickJS-ng was built by.
    /// `None` when `source` is not 64 hex digits.
    pub fn version(&self) -> Option<EngineVersion> {
        Some(EngineVersion {
            semver: self.semver.to_owned(),
            commit: self.commit.to_owned(),
            source: ContentHash::from_hex(self.source)?,
            target: self.target.to_owned(),
            profile: self.profile.to_owned(),
            contract: self.contract.to_owned(),
            c_compiler: QJS_CC.to_owned(),
        })
    }
}

/// Installs `built` as the running engine's version. Installing the same version again is a no-op
/// (a page may build several games); `Err` carries the version already in place when it differs,
/// which means something read [`EngineVersion::current`] before the install. A malformed source
/// installs nothing and returns that version too.
pub fn install_engine(
    built: &BuiltEngine<'_>,
) -> Result<&'static EngineVersion, Box<EngineVersion>> {
    let Some(v) = built.version() else {
        return Err(Box::new(EngineVersion::current().clone()));
    };
    // A refused install hands back the value offered, not the one in place.
    let _ = EngineVersion::install(v.clone());
    let current = EngineVersion::current();
    if *current == v {
        Ok(current)
    } else {
        Err(Box::new(current.clone()))
    }
}

/// Installs the version the calling crate's build script emitted (the `POCKET_ENGINE_*`
/// variables of `crates/pocket-app/build/engine_version.rs`), with the calling crate's version as
/// `semver`; see [`install_engine`].
#[macro_export]
macro_rules! install_engine_version {
    () => {
        $crate::version::install_engine(&$crate::version::BuiltEngine {
            semver: env!("CARGO_PKG_VERSION"),
            commit: env!("POCKET_ENGINE_COMMIT"),
            source: env!("POCKET_ENGINE_SOURCE"),
            target: env!("POCKET_ENGINE_TARGET"),
            profile: env!("POCKET_ENGINE_PROFILE"),
            contract: env!("POCKET_ENGINE_CONTRACT"),
        })
    };
}
