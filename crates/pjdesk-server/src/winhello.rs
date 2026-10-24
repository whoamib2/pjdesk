//! Windows Hello for the PJDesk Server window.
//!
//! Same `UserConsentVerifier` prompt as the client. Failure falls back to the PIN.

use std::sync::mpsc::{self, Receiver};

pub fn spawn_check() -> Receiver<bool> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("winhello".into())
        .spawn(move || {
            let _ = tx.send(available());
        })
        .ok();
    rx
}

pub fn prompt(message: &str) -> bool {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prompt_inner(message))) {
        Ok(true) => {
            tracing::info!("Windows Hello unlocked the PJDesk Server window");
            true
        }
        Ok(false) => {
            tracing::info!("Windows Hello was not accepted. Asking for the PIN.");
            false
        }
        Err(_) => {
            tracing::info!("Windows Hello could not be shown. Asking for the PIN.");
            false
        }
    }
}

#[cfg(not(windows))]
fn available() -> bool {
    false
}

#[cfg(not(windows))]
fn prompt_inner(_message: &str) -> bool {
    false
}

#[cfg(windows)]
fn available() -> bool {
    use windows::Security::Credentials::UI::{UserConsentVerifier, UserConsentVerifierAvailability};
    if apartment().is_err() {
        return false;
    }
    let Ok(op) = UserConsentVerifier::CheckAvailabilityAsync() else {
        return false;
    };
    match op.join() {
        Ok(status) if status == UserConsentVerifierAvailability::Available => {
            tracing::info!("Windows Hello is available for PJDesk Server");
            true
        }
        _ => {
            tracing::info!("Windows Hello is not set up for PJDesk Server. The PIN is the way in.");
            false
        }
    }
}

#[cfg(windows)]
fn prompt_inner(message: &str) -> bool {
    use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier};
    use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    use windows::core::{factory, HSTRING};

    if apartment().is_err() {
        return false;
    }
    let message = HSTRING::from(message);
    let hwnd = unsafe { GetForegroundWindow() };
    let result = if hwnd.0.is_null() {
        UserConsentVerifier::RequestVerificationAsync(&message).and_then(|op| op.join())
    } else {
        match factory::<UserConsentVerifier, IUserConsentVerifierInterop>() {
            Ok(interop) => unsafe {
                interop.RequestVerificationForWindowAsync::<windows_future::IAsyncOperation<UserConsentVerificationResult>>(hwnd, &message)
            }
            .and_then(|op| op.join()),
            Err(_) => UserConsentVerifier::RequestVerificationAsync(&message).and_then(|op| op.join()),
        }
    };
    matches!(result, Ok(UserConsentVerificationResult::Verified))
}

#[cfg(windows)]
fn apartment() -> windows::core::Result<()> {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_ok() || hr.0 == 1 {
        Ok(())
    } else {
        Err(hr.into())
    }
}
