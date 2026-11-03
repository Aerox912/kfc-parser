//! Routes the two proxy DLLs to EML or ShroudForge.
//! EML remains the fallback when its standalone install has no ShroudForge config.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
};

static SELECTED_BACKEND: OnceLock<Backend> = OnceLock::new();

/// The selected proxy owner. EML keeps the original in-process initialization path.
/// ShroudForge receives control through its statically imported `winmm.dll` host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Eml,
    ShroudForge,
    CurrentAssets,
    FullRuntime,
    Failed,
}

/// Reads ShroudForge's existing `modloader` setting once per proxy image.
/// A missing config selects EML, preserving standalone EML installs. A config
/// value of `"eml"` selects EML, while ShroudForge's `"shroudforge"` default
/// selects the statically imported ShroudForge host in feature-enabled builds.
pub fn selected_backend() -> Backend {
    *SELECTED_BACKEND.get_or_init(|| {
        let Some(root) = game_root() else {
            return Backend::Failed;
        };
        choose_backend(&root, current_assets_owned(&root), || {
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
/// EML initialization. `Started` means the selected SF startup was accepted, so
/// the proxy must skip EML initialization. `Failed` aborts that proxy attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handoff {
    EmlSelected,
    Started,
    Failed,
}

/// Keeps the existing EML path; starts the selected SF route after selection.
/// # Safety
/// Called only from a proxy's actual process-attach callback after forwarding
/// initialization; pass its unmodified module and lpReserved arguments.
pub unsafe fn try_start(proxy_module: *mut core::ffi::c_void,
    attach_reserved: *mut core::ffi::c_void) -> Handoff {
    match selected_backend() {
        Backend::Eml => Handoff::EmlSelected,
        Backend::ShroudForge | Backend::FullRuntime => unsafe {
            start_static_host(proxy_module, attach_reserved)
        },
        Backend::CurrentAssets => try_start_shroudforge("shroudforge-assets-bootstrap.dll"),
        Backend::Failed => Handoff::Failed,
    }
}

#[cfg(all(windows, feature = "sf-static-startup"))]
#[link(name = "ShroudForgeWinmm", kind = "dylib")]
unsafe extern "C" {
    fn ShroudforgeStart(proxy_module: *mut core::ffi::c_void,
        attach_reserved: *mut core::ffi::c_void) -> i32;
}

#[cfg(all(windows, feature = "sf-static-startup"))]
unsafe fn start_static_host(proxy: *mut core::ffi::c_void,
    reserved: *mut core::ffi::c_void) -> Handoff {
    if proxy.is_null() || reserved.is_null() { return Handoff::Failed; }
    if unsafe { ShroudforgeStart(proxy, reserved) } != 0 {
        Handoff::Started
    } else { Handoff::Failed }
}

#[cfg(not(all(windows, feature = "sf-static-startup")))]
unsafe fn start_static_host(_proxy: *mut core::ffi::c_void,
    _reserved: *mut core::ffi::c_void) -> Handoff {
    // Standalone EML has no SF dependency and never dynamically loads winmm.
    Handoff::Failed
}

/// Loads ShroudForge's installed bootstrap. This is only called for the
/// ShroudForge backend. EML initialization is handled by the proxy instead.
/// If either release file is missing or loading fails, the proxy attach fails.
/// This minimal bridge does not start the updater or change parser behavior.
#[cfg(all(windows, feature = "sf-static-startup"))]
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
#[cfg(not(all(windows, feature = "sf-static-startup")))]
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

const MAX_POLICY_BYTES: u64 = 1024 * 1024;

// Route discriminator only. The runtime owns complete schema/receipt validation.
// Marker ownership never falls through to EML on missing or invalid policy.
fn prepared_backend(root: &Path) -> Backend {
    let read = || -> Option<u64> {
        let file = fs::File::open(root.join("shroudforge/config/current-assets.json")).ok()?;
        let mut bytes = Vec::new();
        file.take(MAX_POLICY_BYTES + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 > MAX_POLICY_BYTES { return None; }
        let policy: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        policy.as_object()?.get("schemaVersion")?.as_u64()
    };
    match read() {
        Some(1..=5) => Backend::CurrentAssets,
        Some(6) => Backend::FullRuntime,
        _ => Backend::Failed,
    }
}

fn choose_backend(root: &Path, ownership: std::io::Result<bool>, legacy: impl FnOnce() -> Backend) -> Backend {
    match ownership {
        Ok(true) => prepared_backend(root),
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
                choose_backend(Path::new("unused"), Err(std::io::Error::from(kind)), || panic!(
                    "must not read legacy selector"
                )),
                Backend::Failed
            );
        }
        assert_eq!(
            choose_backend(Path::new("missing-policy"), Ok(true), || panic!("owned path must not downgrade")),
            Backend::Failed
        );
        assert_eq!(choose_backend(Path::new("unused"), Ok(false), || Backend::Eml), Backend::Eml);
        assert_eq!(
            choose_backend(Path::new("unused"), Ok(false), || Backend::ShroudForge),
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
    #[test]
    fn prepared_schema_routes_and_invalid_policy_refuses() {
        let root = std::env::temp_dir().join(format!("sf-bridge-policy-{}", std::process::id()));
        let config = root.join("shroudforge/config");
        fs::create_dir_all(&config).unwrap();
        let policy = config.join("current-assets.json");
        for schema in 1..=6 {
            fs::write(&policy, format!(r#"{{"schemaVersion":{schema}}}"#)).unwrap();
            assert_eq!(choose_backend(&root, current_assets_owned(&root), || panic!("no downgrade")),
                if schema == 6 { Backend::FullRuntime } else { Backend::CurrentAssets });
        }
        for invalid in ["", "{", "null", "[]", "{}", r#"{"schemaVersion":7}"#,
            r#"{"schemaVersion":"6"}"#, r#"{"schemaVersion":-1}"#] {
            fs::write(&policy, invalid).unwrap();
            assert_eq!(prepared_backend(&root), Backend::Failed);
        }
        fs::write(&policy, vec![b' '; MAX_POLICY_BYTES as usize + 1]).unwrap();
        assert_eq!(prepared_backend(&root), Backend::Failed);
        fs::remove_file(&policy).unwrap();
        for marker in ["current-assets-state.json", "current-assets-pending.json"] {
            fs::write(config.join(marker), b"{}").unwrap();
            assert_eq!(choose_backend(&root, current_assets_owned(&root), || panic!("no downgrade")), Backend::Failed);
            fs::remove_file(config.join(marker)).unwrap();
        }
        // A directory at the policy path is unreadable as a policy, never EML.
        fs::create_dir(&policy).unwrap();
        assert_eq!(choose_backend(&root, current_assets_owned(&root), || panic!("no downgrade")), Backend::Failed);
        fs::remove_dir(&policy).unwrap();
    }
    #[cfg(not(feature = "sf-static-startup"))]
    #[test]
    fn featureless_build_refuses_sf_without_loading_a_host() {
        assert_eq!(unsafe { start_static_host(core::ptr::null_mut(), core::ptr::null_mut()) }, Handoff::Failed);
        assert_eq!(try_start_shroudforge("shroudforge-assets-bootstrap.dll"), Handoff::Failed);
    }
}
