//! Windows Hello for the lock screen.
//!
//! Uses `Windows.Security.Credentials.UI.UserConsentVerifier`. A desktop
//! window also asks through `IUserConsentVerifierInterop` so the prompt is
//! tied to this process's window. Failure falls back to the PJDesk PIN.

use std::sync::mpsc::{self, Receiver};

/// Starts a check for fingerprint, face, or a Windows Hello PIN.
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

/// True when the person at this computer proved it with Windows Hello.
pub fn prompt(message: &str) -> bool {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prompt_inner(message))) {
        Ok(true) => {
            tracing::info!("Windows Hello unlocked the window");
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
    if let Err(err) = apartment() {
        tracing::info!("Windows Hello is not available ({err})");
        return false;
    }
    let op = match UserConsentVerifier::CheckAvailabilityAsync() {
        Ok(op) => op,
        Err(err) => {
            tracing::info!("Windows Hello is not available ({err})");
            return false;
        }
    };
    match op.join() {
        Ok(status) if status == UserConsentVerifierAvailability::Available => {
            tracing::info!("Windows Hello is available");
            true
        }
        Ok(_) => {
            tracing::info!("Windows Hello is not set up. The PIN is the way in.");
            false
        }
        Err(err) => {
            tracing::info!("Windows Hello is not available ({err})");
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
            Ok(interop) => unsafe { interop.RequestVerificationForWindowAsync::<windows_future::IAsyncOperation<UserConsentVerificationResult>>(hwnd, &message) }
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
        // S_FALSE (1): this thread was already initialized.
        Ok(())
    } else {
        Err(hr.into())
    }
}
