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
}

/// Reads ShroudForge's existing `modloader` setting once per process.
/// A missing config selects EML, preserving standalone EML installs. A config
/// value of `"eml"` selects EML, while ShroudForge's `"shroudforge"` default
/// selects the external ShroudForge bootstrap.
pub fn selected_backend() -> Backend {
    *SELECTED_BACKEND.get_or_init(|| {
        let Some(root) = game_root() else {
            return Backend::Eml;
        };
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
}

/// Result of backend dispatch. `EmlSelected` tells the proxy to run its original
/// EML initialization. `Started` means the ShroudForge bootstrap admitted startup, so
/// the proxy must skip EML initialization. `Failed` aborts that proxy attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handoff {
    EmlSelected,
    Started,
    Failed,
}

/// Dispatch without process-attach provenance can select EML, but cannot start
/// ShroudForge's pre-entry bootstrap. Proxies use `try_start_from_process_attach`.
pub fn try_start() -> Handoff {
    match selected_backend() {
        Backend::Eml => Handoff::EmlSelected,
        Backend::ShroudForge => Handoff::Failed,
    }
}

/// Route an actual proxy DllMain attachment. Only the OS-provided non-null
/// reserved pointer can admit ShroudForge's pre-entry bootstrap. EML behavior
/// remains the existing proxy-owned route.
///
/// # Safety
/// `module` and `reserved` must be the original parameters of the current
/// DLL_PROCESS_ATTACH callback. Call synchronously from that callback only.
pub unsafe fn try_start_from_process_attach(
    module: *mut std::ffi::c_void,
    reserved: *mut std::ffi::c_void,
) -> Handoff {
    match selected_backend() {
        Backend::Eml => Handoff::EmlSelected,
        Backend::ShroudForge => {
            if module.is_null() || reserved.is_null() {
                return Handoff::Failed;
            }
            unsafe { try_start_shroudforge(module, reserved) }
        }
    }
}

/// Loads ShroudForge's installed bootstrap. This is only called for the
/// ShroudForge backend. EML initialization is handled by the proxy instead.
/// If either release file is missing or loading fails, the proxy attach fails.
/// This minimal bridge does not start the updater or change parser behavior.
#[cfg(windows)]
unsafe fn try_start_shroudforge(
    module: *mut std::ffi::c_void,
    reserved: *mut std::ffi::c_void,
) -> Handoff {
    use std::{cell::Cell, ffi::c_void, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::System::{
        LibraryLoader::{GetProcAddress, LoadLibraryW},
        Threading::GetCurrentThreadId,
    };

    struct AttachContext {
        module: *mut c_void,
        thread: u32,
        reserved: *mut c_void,
        active: Cell<bool>,
    }
    unsafe extern "system" fn verify(
        context: *const c_void,
        module: *mut c_void,
        thread: u32,
    ) -> i32 {
        if context.is_null() || unsafe { GetCurrentThreadId() } != thread {
            return 0;
        }
        let context = unsafe { &*context.cast::<AttachContext>() };
        i32::from(
            context.active.get()
                && !context.reserved.is_null()
                && context.module == module
                && context.thread == thread,
        )
    }
    #[repr(C)]
    struct StaticProxyAttachProof {
        size: u32,
        version: u32,
        proxy_module: *mut c_void,
        attach_thread: u32,
        context: *const c_void,
        verify: Option<unsafe extern "system" fn(*const c_void, *mut c_void, u32) -> i32>,
    }
    type Start = unsafe extern "system" fn(*const StaticProxyAttachProof) -> i32;

    let Some(root) = game_root() else {
        return Handoff::Failed;
    };
    let bootstrap = root.join("winmm.dll");
    let runtime = root.join("shroudforge/shroudforge-runtime.dll");
    if !bootstrap.is_file() || !runtime.is_file() {
        return Handoff::Failed;
    }
    let wide_path = bootstrap
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();

    let bootstrap = unsafe { LoadLibraryW(wide_path.as_ptr()) };
    if bootstrap.is_null() {
        return Handoff::Failed;
    }
    let Some(start) = (unsafe {
        GetProcAddress(
            bootstrap,
            c"ShroudforgeStartFromStaticProxy".as_ptr().cast(),
        )
    }) else {
        return Handoff::Failed;
    };
    let context = AttachContext {
        module,
        thread: unsafe { GetCurrentThreadId() },
        reserved,
        active: Cell::new(true),
    };
    let proof = StaticProxyAttachProof {
        size: std::mem::size_of::<StaticProxyAttachProof>() as u32,
        version: 1,
        proxy_module: module,
        attach_thread: context.thread,
        context: std::ptr::from_ref(&context).cast(),
        verify: Some(verify),
    };
    let start: Start = unsafe { std::mem::transmute(start) };
    // The callback/context never escapes this synchronous outer DllMain scope.
    let started = unsafe { start(&proof) } != 0;
    context.active.set(false);
    if started {
        Handoff::Started
    } else {
        Handoff::Failed
    }
}

/// Non-Windows builds cannot load the ShroudForge DLL. EML builds remain usable.
#[cfg(not(windows))]
unsafe fn try_start_shroudforge(_: *mut std::ffi::c_void, _: *mut std::ffi::c_void) -> Handoff {
    Handoff::Failed
}

/// The game executable's directory is the install root for both proxy backends.
fn game_root() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    executable.parent().map(Path::to_path_buf)
}
