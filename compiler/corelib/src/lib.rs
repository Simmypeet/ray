//! Filesystem-independent inputs for the mandatory compiler core library.

use std::path::{Path, PathBuf};

use rayc_qbice::InputSession;
use rayc_source_file::{LocalSourceID, SourceFile};
use rayc_target::TargetID;

/// The single virtual source path, independent of the checkout and working
/// directory.
pub const SOURCE_PATH: &str = "__ray_core__/core.ray";
/// Source shipped with this compiler; installed as tracked input on every
/// initialization.
pub const SOURCE: &str = include_str!("../core.ray");
/// Stable editor location for diagnostics referring to the embedded source.
pub const SOURCE_URI: &str = "ray-core:/core.ray";

/// Supplies core inputs without committing the caller's transaction or changing
/// target maps.
pub async fn initialize(session: &mut InputSession) {
    let target_id = TargetID::CORE;
    let path = session.intern_unsized::<Path, _>(PathBuf::from(SOURCE_PATH));
    session.set_input(rayc_target::RootFilePathKey { target_id }, path.clone()).await;
    session.set_input(rayc_target::NameKey { target_id }, session.intern_unsized("core")).await;
    session
        .set_input(rayc_target::LinkKey { target_id }, session.intern(std::iter::empty().collect()))
        .await;
    session.set_input(rayc_target::IRVerificationKey { target_id }, false).await;
    session
        .set_input(
            rayc_source_file::Key { path: path.clone(), target_id },
            Ok(SourceFile::from_str(SOURCE, path.clone())),
        )
        .await;
    // One local source, qualified by CORE. Neither text nor filesystem state
    // affects its identity.
    session
        .set_input(
            rayc_source_file::StablePathIDKey { path, target_id },
            Ok(LocalSourceID::new(0, 0)),
        )
        .await;
}
