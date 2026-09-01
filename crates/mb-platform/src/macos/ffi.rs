//! Raw bindings to the macOS frameworks used by [`super::MacPlatform`].
//!
//! All `unsafe` in the macOS backend is confined to this module. Each binding is
//! a direct declaration of a documented Apple C function, and every wrapper below
//! is a safe function whose contract is discharged by a `SAFETY` comment.
//!
//! The Rust bindings needed here are not provided by the `core-graphics` or
//! `core-foundation` crates: the Accessibility and Input Monitoring gates live in
//! ApplicationServices and IOKit respectively, and neither has a maintained
//! binding crate worth adding a dependency for.

// Justification for the workspace-wide `unsafe_code` lint: calling C from Rust
// requires it, and there is no safe alternative for these APIs. The scope is one
// module, and nothing outside it is unsafe.
#![allow(unsafe_code)]

use crate::PermissionStatus;
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;
use std::ptr;

/// CoreFoundation's `Boolean`, an unsigned char.
type Boolean = u8;

/// `IOHIDRequestType`. `kIOHIDRequestTypeListenEvent` is the observe-input gate.
const REQUEST_TYPE_LISTEN_EVENT: u32 = 1;

/// `IOHIDAccessType` values.
const ACCESS_GRANTED: u32 = 0;
const ACCESS_DENIED: u32 = 1;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    /// `Boolean AXIsProcessTrusted(void)`
    fn AXIsProcessTrusted() -> Boolean;
    /// `Boolean AXIsProcessTrustedWithOptions(CFDictionaryRef options)`
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
    /// Option key requesting that the system show the Accessibility prompt.
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    /// `IOHIDAccessType IOHIDCheckAccess(IOHIDRequestType requestType)`
    fn IOHIDCheckAccess(request: u32) -> u32;
    /// `Boolean IOHIDRequestAccess(IOHIDRequestType requestType)`
    fn IOHIDRequestAccess(request: u32) -> Boolean;
}

#[link(name = "SystemConfiguration", kind = "framework")]
unsafe extern "C" {
    /// `CFStringRef SCDynamicStoreCopyComputerName(SCDynamicStoreRef, CFStringEncoding *)`
    fn SCDynamicStoreCopyComputerName(store: *const c_void, encoding: *mut u32) -> CFStringRef;
}

/// Returns whether this process holds the Accessibility permission.
///
/// Side-effect free, so it is safe to poll from the dashboard.
pub fn is_process_trusted() -> bool {
    // SAFETY: `AXIsProcessTrusted` takes no arguments, has no preconditions, and
    // returns a `Boolean` by value. There is nothing to invalidate.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Asks macOS to display the Accessibility permission prompt.
///
/// The prompt appears at most once per application per user; afterwards macOS
/// silently does nothing, which is why the UI must always also offer a link to
/// System Settings.
pub fn prompt_for_accessibility() {
    // SAFETY: reading an immutable `CFStringRef` constant exported by
    // ApplicationServices. `wrap_under_get_rule` retains it, matching the Get
    // Rule that applies to a framework-owned constant.
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);

    // SAFETY: `options` is a valid CFDictionary that outlives the call, and the
    // function borrows it under the Get Rule without taking ownership.
    unsafe {
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef());
    }
}

/// Returns the Input Monitoring permission status.
pub fn input_monitoring_status() -> PermissionStatus {
    // SAFETY: `IOHIDCheckAccess` takes a plain enum value and returns one. It is
    // documented as safe to call from any thread and does not prompt.
    let access = unsafe { IOHIDCheckAccess(REQUEST_TYPE_LISTEN_EVENT) };
    match access {
        ACCESS_GRANTED => PermissionStatus::Granted,
        ACCESS_DENIED => PermissionStatus::Denied,
        // `kIOHIDAccessTypeUnknown` means never requested. Anything else is a
        // value Apple has added since; both are reported honestly rather than
        // guessed at, because assuming a grant produces a silent capture failure.
        2 => PermissionStatus::NotDetermined,
        _ => PermissionStatus::Unknown,
    }
}

/// Asks macOS to display the Input Monitoring permission prompt.
pub fn request_input_monitoring() {
    // SAFETY: as `IOHIDCheckAccess`. The return value indicates whether the
    // permission is already held, which the caller re-queries rather than trusts.
    unsafe {
        IOHIDRequestAccess(REQUEST_TYPE_LISTEN_EVENT);
    }
}

/// Returns the user-facing computer name, e.g. `Anh's MacBook Pro`.
///
/// This is the name shown in Sharing settings and the one users recognise, unlike
/// the DNS hostname, which is usually a mangled form of it.
pub fn computer_name() -> Option<String> {
    // SAFETY: a NULL store makes SCDynamicStore create a transient session for
    // the call, and a NULL encoding pointer is documented as "not interested".
    // Both are explicitly permitted by the SystemConfiguration API.
    let raw = unsafe { SCDynamicStoreCopyComputerName(ptr::null(), ptr::null_mut()) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: the function name contains `Copy`, so the Create Rule applies and
    // this call transfers ownership to the CFString wrapper, which releases it on
    // drop. The pointer was null-checked immediately above.
    let name = unsafe { CFString::wrap_under_create_rule(raw) };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computer_name_is_available_on_a_real_mac() {
        let name = computer_name().expect("SCDynamicStoreCopyComputerName returned NULL");
        assert!(!name.trim().is_empty());
    }

    #[test]
    fn permission_probes_are_callable_and_side_effect_free() {
        // Called twice: a status probe that prompted or mutated state would be a
        // serious bug, since the dashboard polls these while it is open.
        let first = (is_process_trusted(), input_monitoring_status());
        let second = (is_process_trusted(), input_monitoring_status());
        assert_eq!(first, second);
    }

    #[test]
    fn input_monitoring_status_is_a_recognised_value() {
        assert!(
            !matches!(input_monitoring_status(), PermissionStatus::NotRequired),
            "macOS always gates Input Monitoring"
        );
    }
}
