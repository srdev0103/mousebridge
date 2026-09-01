//! Windows platform backend.
//!
//! # Validation status
//!
//! **Compile-checked only.** This module is type-checked from the development
//! Mac with `cargo check --target x86_64-pc-windows-msvc`, which proves the
//! signatures and types are right and proves nothing about runtime behaviour.
//! Every item here is listed in `docs/platform-validation.md` and must be run on
//! real Windows before it is described as working.
//!
//! # Coordinate space
//!
//! Windows reports monitor rectangles in the virtual-screen coordinate space, in
//! physical pixels, for a Per-Monitor-V2 DPI-aware process. Those values are
//! reported as-is, with the per-monitor scale factor alongside. See
//! `docs/adr/0001-display-coordinate-space.md` for why they are not pre-divided
//! by the scale factor here.

#![allow(unsafe_code)]

use crate::host::sanitize_host_name;
use crate::{
    DisplayInfo, DisplayLayout, HostInfo, Permission, PermissionStatus, Platform, PlatformError,
};
use mb_types::{Arch, LogicalRect, OsKind, Scale, ScreenId};
use std::ffi::c_void;
use tracing::warn;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::PWSTR;

/// Windows applies no privacy gate to the low-level input hooks this application
/// uses, so there is nothing to request. The elevated-window and secure-desktop
/// limitations described in the architecture are a privilege boundary rather than
/// a permission, and cannot be resolved by asking the user.
const REQUIRED: &[Permission] = &[];

/// Reference DPI. Windows defines 96 DPI as 100% scaling.
const REFERENCE_DPI: f64 = 96.0;

/// Windows implementation of [`Platform`].
#[derive(Debug, Default)]
pub struct WindowsPlatform {
    _private: (),
}

impl WindowsPlatform {
    /// Builds the backend.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

/// One monitor, as gathered by the enumeration callback.
struct RawMonitor {
    rect: RECT,
    is_primary: bool,
    device: String,
    dpi: u32,
}

/// Receives one monitor per invocation from `EnumDisplayMonitors`.
///
/// # Safety
///
/// Called by Windows with a valid `HMONITOR`. `lparam` must be the `*mut
/// Vec<RawMonitor>` passed to `EnumDisplayMonitors`, which is guaranteed by the
/// single call site in [`WindowsPlatform::displays`], where the vector outlives
/// the enumeration.
unsafe extern "system" fn monitor_callback(
    monitor: HMONITOR,
    _hdc: HDC,
    _clip: *mut RECT,
    lparam: LPARAM,
) -> windows::core::BOOL {
    // SAFETY: the pointer round-trips from the single call site below, where it
    // is taken from a live `Vec` that outlives this enumeration.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<RawMonitor>) };

    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = u32::try_from(size_of::<MONITORINFOEXW>()).unwrap_or(0);

    // SAFETY: `info` is a correctly sized, correctly initialised MONITORINFOEXW,
    // and `monitor` was supplied by the OS.
    let ok = unsafe { GetMonitorInfoW(monitor, std::ptr::from_mut(&mut info).cast()) };
    if !ok.as_bool() {
        warn!("GetMonitorInfoW failed for a monitor; skipping it");
        // Continue enumerating: one unreadable monitor must not hide the others.
        return true.into();
    }

    let mut dpi_x = 0u32;
    let mut dpi_y = 0u32;
    // SAFETY: both out-pointers are valid for the duration of the call.
    let dpi = match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
    {
        Ok(()) => dpi_x,
        Err(e) => {
            // Falling back to 96 keeps the monitor usable at 100% scaling rather
            // than dropping it from the topology entirely.
            warn!(error = %e, "GetDpiForMonitor failed; assuming 96 DPI");
            96
        }
    };

    let device_end = info
        .szDevice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.szDevice.len());
    let device = String::from_utf16_lossy(&info.szDevice[..device_end]);

    out.push(RawMonitor {
        rect: info.monitorInfo.rcMonitor,
        is_primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        device,
        dpi,
    });
    true.into()
}

impl Platform for WindowsPlatform {
    fn host(&self) -> Result<HostInfo, PlatformError> {
        Ok(HostInfo {
            name: sanitize_host_name(&computer_name()?),
            os: OsKind::Windows,
            arch: Arch::current(),
            os_version: os_version(),
        })
    }

    fn displays(&self) -> Result<DisplayLayout, PlatformError> {
        let mut raw: Vec<RawMonitor> = Vec::new();

        // SAFETY: the callback matches MONITORENUMPROC, and `lparam` points at
        // `raw`, which is alive for the whole call and not aliased meanwhile.
        let ok = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(monitor_callback),
                LPARAM(std::ptr::from_mut(&mut raw) as isize),
            )
        };
        if !ok.as_bool() {
            return Err(PlatformError::os_call(
                "EnumDisplayMonitors",
                "enumeration was interrupted",
            ));
        }

        let mut out = Vec::with_capacity(raw.len());
        for (index, m) in raw.into_iter().enumerate() {
            let width = f64::from(m.rect.right - m.rect.left);
            let height = f64::from(m.rect.bottom - m.rect.top);
            let Some(bounds) = LogicalRect::from_parts(
                f64::from(m.rect.left),
                f64::from(m.rect.top),
                width,
                height,
            ) else {
                warn!(device = %m.device, "skipping monitor with degenerate bounds");
                continue;
            };

            out.push(DisplayInfo {
                // Windows has no stable small integer per monitor, so the index
                // within this enumeration is used. DisplayLayout sorts by
                // position, which keeps the assignment stable as long as the
                // physical arrangement does not change.
                id: ScreenId(u32::try_from(index).unwrap_or(u32::MAX)),
                bounds,
                scale: Scale::new(f64::from(m.dpi) / REFERENCE_DPI).unwrap_or(Scale::ONE),
                is_primary: m.is_primary,
                name: Some(m.device),
            });
        }

        Ok(DisplayLayout::new(out))
    }

    fn permission_status(&self, _permission: Permission) -> PermissionStatus {
        PermissionStatus::NotRequired
    }

    fn request_permission(&self, _permission: Permission) -> Result<(), PlatformError> {
        Ok(())
    }

    fn open_permission_settings(&self, _permission: Permission) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported {
            what: "privacy settings pane",
        })
    }

    fn required_permissions(&self) -> &'static [Permission] {
        REQUIRED
    }
}

/// Returns the machine's name.
///
/// Windows shows the DNS hostname as "Device name" in Settings, so unlike macOS
/// there is no separate friendly name to prefer.
fn computer_name() -> Result<String, PlatformError> {
    let mut size = 0u32;
    // First call reports the required buffer size and is expected to fail with
    // ERROR_MORE_DATA, so its result is deliberately discarded.
    // SAFETY: a null buffer with a valid size pointer is the documented way to
    // query the required length.
    let _ = unsafe { GetComputerNameExW(ComputerNameDnsHostname, None, &mut size) };
    if size == 0 {
        return Err(PlatformError::bad_response(
            "GetComputerNameExW",
            "reported a zero-length name",
        ));
    }

    let mut buffer = vec![0u16; size as usize];
    // SAFETY: `buffer` has `size` elements, which is the length the OS asked for,
    // and `size` remains valid for the call.
    unsafe {
        GetComputerNameExW(
            ComputerNameDnsHostname,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut size,
        )
    }
    .map_err(|e| PlatformError::os_call("GetComputerNameExW", e.to_string()))?;

    buffer.truncate(size as usize);
    Ok(String::from_utf16_lossy(&buffer))
}

/// Returns the Windows version as `major.minor.build`.
///
/// `RtlGetVersion` is used rather than `GetVersionEx` because the latter reports
/// a shimmed version for applications without a compatibility manifest, which
/// would make support output actively misleading.
fn os_version() -> String {
    use windows::Wdk::System::SystemServices::RtlGetVersion;
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;

    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: u32::try_from(size_of::<OSVERSIONINFOW>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: `info` is a correctly sized OSVERSIONINFOW that outlives the call.
    let status = unsafe { RtlGetVersion(&mut info) };
    if status.is_ok() {
        format!(
            "{}.{}.{}",
            info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
        )
    } else {
        "unknown".to_owned()
    }
}

// A `HWND` import is not needed at runtime but keeps the intent explicit if the
// enumeration is later scoped to a window; silence the unused warning.
const _: Option<HWND> = None;
const _: Option<*const c_void> = None;
