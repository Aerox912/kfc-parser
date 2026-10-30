//! Routes the two proxy DLLs to EML or ShroudForge.
//! EML remains the fallback when its standalone install has no ShroudForge config.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

static SELECTED_BACKEND: OnceLock<Backend> = OnceLock::new();

/// The selected proxy owner. EML keeps the original in-process initialization path.
/// ShroudForge receives control through its installed `winmm.dll` bootstrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Eml,
    ShroudForge,
    CurrentAssets,
    Failed,
}

/// Reads ShroudForge's existing `modloader` setting once per process.
/// A missing config selects EML, preserving standalone EML installs. A config
/// value of `"eml"` selects EML, while ShroudForge's `"shroudforge"` default
/// selects the external ShroudForge bootstrap.
pub fn selected_backend() -> Backend {
    *SELECTED_BACKEND.get_or_init(|| {
        let Some(root) = game_root() else {
            return Backend::Failed;
        };
        choose_backend(current_assets_owned(&root), || {
            let config_path = root.join("shroudforge/config/modloader-config.json");
            let Ok(bytes) = fs::read(config_path) else {
                return Backend::Eml;
            };
            let config = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
            if config
                .as_ref()
                .and_then(|value| value.get("modloader"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|backend| backend.eq_ignore_ascii_case("eml"))
            {
                Backend::Eml
            } else {
                Backend::ShroudForge
            }
        })
    })
}

/// Result of backend dispatch. `EmlSelected` tells the proxy to run its original
/// EML initialization. `Started` means the ShroudForge bootstrap was loaded, so
/// the proxy must skip EML initialization. `Failed` aborts that proxy attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handoff {
    EmlSelected,
    Started,
    Failed,
}

/// Keeps the existing EML path in the proxy and loads ShroudForge only when selected.
pub fn try_start() -> Handoff {
    match selected_backend() {
        Backend::Eml => Handoff::EmlSelected,
        Backend::ShroudForge => try_start_shroudforge("winmm.dll"),
        Backend::CurrentAssets => try_start_shroudforge("shroudforge-assets-bootstrap.dll"),
        Backend::Failed => Handoff::Failed,
    }
}

/// Loads ShroudForge's installed bootstrap. This is only called for the
/// ShroudForge backend. EML initialization is handled by the proxy instead.
/// If either release file is missing or loading fails, the proxy attach fails.
/// This minimal bridge does not start the updater or change parser behavior.
#[cfg(windows)]
fn try_start_shroudforge(bootstrap_name: &str) -> Handoff {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;

    let Some(root) = game_root() else {
        return Handoff::Failed;
    };
    // The once-per-process decision includes the bootstrap identity; do not
    // rescan ownership and accidentally downgrade it to the legacy bootstrap.
    let bootstrap = root.join(bootstrap_name);
    let runtime = root.join("shroudforge/shroudforge-runtime.dll");
    if !bootstrap.is_file() || !runtime.is_file() {
        return Handoff::Failed;
    }
    let wide_path = bootstrap
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();

    if unsafe { LoadLibraryW(wide_path.as_ptr()) }.is_null() {
        Handoff::Failed
    } else {
        Handoff::Started
    }
}

/// Non-Windows builds cannot load the ShroudForge DLL. EML builds remain usable.
#[cfg(not(windows))]
fn try_start_shroudforge(_bootstrap_name: &str) -> Handoff {
    Handoff::Failed
}

/// The game executable's directory is the install root for both proxy backends.
fn game_root() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    executable.parent().map(Path::to_path_buf)
}

fn current_assets_owned(root: &Path) -> std::io::Result<bool> {
    for name in [
        "current-assets.json",
        "current-assets-state.json",
        "current-assets-pending.json",
    ] {
        if root.join("shroudforge/config").join(name).try_exists()? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn choose_backend(ownership: std::io::Result<bool>, legacy: impl FnOnce() -> Backend) -> Backend {
    match ownership {
        Ok(true) => Backend::CurrentAssets,
        Ok(false) => legacy(),
        Err(_) => Backend::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_ownership_never_selects_eml_or_a_bootstrap() {
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::Other,
        ] {
            assert_eq!(
                choose_backend(Err(std::io::Error::from(kind)), || panic!(
                    "must not read legacy selector"
                )),
                Backend::Failed
            );
        }
        assert_eq!(
            choose_backend(Ok(true), || panic!("owned path must not downgrade")),
            Backend::CurrentAssets
        );
        assert_eq!(choose_backend(Ok(false), || Backend::Eml), Backend::Eml);
        assert_eq!(
            choose_backend(Ok(false), || Backend::ShroudForge),
            Backend::ShroudForge
        );
    }
    #[test]
    fn only_proven_absence_or_owned_markers_select_a_route() {
        let root = std::env::temp_dir().join(format!("sf-bridge-ownership-{}", std::process::id()));
        std::fs::create_dir_all(root.join("shroudforge/config")).unwrap();
        assert!(!current_assets_owned(&root).unwrap());
        for name in [
            "current-assets.json",
            "current-assets-state.json",
            "current-assets-pending.json",
        ] {
            let marker = root.join("shroudforge/config").join(name);
            std::fs::write(&marker, b"{}").unwrap();
            assert!(current_assets_owned(&root).unwrap());
            std::fs::remove_file(marker).unwrap();
        }
    }
}
