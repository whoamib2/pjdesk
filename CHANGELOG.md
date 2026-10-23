# Changelog

## 0.6.15

Windows and Android are 0.6.15 (versionCode 61500). cloud/ did not change, so no deploy. Protocol stays 3. No tag, no release, and no GitHub Actions. The Windows client needs 0.6.15 for View phone. The phone build only carries the version bump.

* View phone failed on Windows with "The handle is invalid. (os error 6)" and only an "Opening" toast. The window starts the viewer hidden, with no password and no console, and the viewer then tried to prompt for a password on the console. A view request now sends no password and never prompts, a viewer without a console stops with a clear message instead, and the window starts every viewer with null standard handles.

## 0.6.14

Android is 0.6.14 (versionCode 61400). Windows and cloud/ did not change. Protocol stays 3. No tag, no release, and no GitHub Actions. If pjdesk.spader.us:21200 is already the active address, do not run the 0.6.1 slot-clearing SQL. Leave Require signed-in accounts off. Do not delete cloud-sync.json or cloud-state.json. The PJDesk Server account sign-in stays persistent. The phone needs 0.6.14. The Windows client does not.

* The session menu button did not receive taps. It used a drag detector that only opens after the finger moves, and it opened only when that move stayed under 12 pixels. On a Pixel the touch slop is larger than 12 pixels, so a tap did nothing and a drag only moved the bubble. The gear, the ⋮ bubble, End, and the display buttons are now real buttons, 56dp, drawn above the picture. The gesture layer does not take a touch that starts on one of them. They sit inside the safe-drawing and system-gesture insets, and a press turns the button white-edged. The gear next to End and the bubble both open the same sheet.
* The sheet has Input (touch or mouse, pointer speed, scroll speed, invert scroll, acceleration, drag lock, local cursor), Display (fit, original, stretch, Best / Balanced / Speed / Custom with frame rate and bitrate, view only, quality monitor, blank the other screen when the host allows it), Keyboard (keyboard, map or translate, Ctrl+Alt+Del, lock), and Session (clipboard sync, send clipboard, their folder, send a file, restart when allowed, idle timeout, disconnect). Changes apply while the session is open. They are remembered as the default for every computer, or for that computer only. Settings → Remote control has the same controls. Remote audio is not played on this phone yet. The remote cursor stays in the picture; this phone cannot hide it.

## 0.6.13

Android is 0.6.13 (versionCode 61300). Windows is 0.6.13 as well: the ask-to-connect prompt, the taskbar notice, and View phone are in the Windows client and the service-hosted host. cloud/ changed: migration `0014_hardware_id.sql` stores a hash of the device id and drops a stale PJDesk id when the same phone registers again. Protocol stays 3. No tag, no release, and no GitHub Actions. If pjdesk.spader.us:21200 is already the active address, do not run the 0.6.1 slot-clearing SQL. Leave Require signed-in accounts off. Do not delete cloud-sync.json or cloud-state.json. The PJDesk Server account sign-in stays persistent. Install the server before the clients only if you are replacing the server binary; 0014 is the account service, so on the PC, in cloud\, run `cloud\deploy.cmd` before relying on the duplicate cleanup. The Windows client and the service-hosted host both need this build for the Allow prompt and the taskbar notice. The phone needs 0.6.13 to send its account and device name.

* Saved computers is a compact list. Each row is about one line of name plus `902 488 771 · Win 11 · 3d ago`, with the OS icon and a status dot. Tap the row or the connect mark. Rename, the signed-in user, and Remove are in the overflow. This phone is a small tag at the bottom. Search is an icon that opens a field. Sort and filter are one menu. Recently used is the default and is still remembered. The sync sentence moved to Settings. Home uses the same tighter spacing.
* Remote control has a pointer speed from 0.25× to 2×, default 0.6×, and acceleration off by default. Both are in Settings → Remote control and in the session menu, and they are remembered. A one-finger trackpad drag only moves the pointer. Holding the button takes the Drag control or a tap then a press-and-drag. A zoomed touch-mode pan does not send mouse-down. Display buttons 1, 2, 3 stay on screen when there is more than one display. End is a red X, 48dp, inset from the edges: tap asks "Disconnect from {name}?", hold ends immediately. Android back asks the same question. Disconnect on the controller notification ends the session, and the other computer sees it.
* One idle setting, Off / 1 / 5 / 15 minutes, default 1 minute, applies in the foreground, in the background, and with the screen off. The foreground shows "Disconnecting in 10s, tap to stay" for the last 10 seconds. Background and screen-off disconnect at the timeout with no banner. Swiping the app away ends the controller session and leaves the phone registered. Reopening shows the session only while it is still connected, with a working End. Otherwise it shows Home.
* Connect with no saved password offers Enter password or Ask the person at {name}. The password dialog has Ask to connect instead. Asking shows "Waiting for {name} to accept…" with Cancel and a 60 second timeout, then Accepted, Declined, or No answer. On Windows, Allow or Deny is brought to the front of the logged-on user's PJDesk window, focused, with a sound, and it names the account or the computer. If nobody is logged on or that window is not running, the host answers "No one is available to accept" immediately. A permanent password still skips the prompt.
* When someone connects, the controlled PC shows who it is for about 2 seconds, then minimizes to a real taskbar button that can be restored for the session and Disconnect. If PJDesk is open in the logged-on session, that window owns the notice while the host runs as a service. The phone sends its pjdesk.com account and its device name on connect, on ask, and as the visitor card.
* An Android row in the Windows Saved list uses View phone. The phone still asks Allow or Deny unless the account, that computer, or a permanent password already allows it, then Android's screen-share consent, then the stream. The Windows viewer Phone menu still has Back, Home, Recents, volume, and power. A failed view says Phone declined, Screen sharing not allowed on the phone, or Phone app is closed or asleep, and it does not keep retrying. The phone stays registered while it is locked or in the background. App lock does not cover the view-request screen.

## 0.6.12

Android is 0.6.12 (versionCode 61200). The workspace version is 0.6.12 with it. Windows clients and the server are 0.6.12 as well. cloud/ did not change: the account already stores the server's LAN address. Protocol stays 3. No tag, no release, and no GitHub Actions. If pjdesk.spader.us:21200 is already the active address, do not run the 0.6.1 slot-clearing SQL. Leave Require signed-in accounts off. Do not delete cloud-sync.json or cloud-state.json. The PJDesk Server account sign-in stays persistent. Install the server before the clients, then install the clients, so a phone on the same Wi-Fi hears the server's LAN address. There is no new migration. On Windows, build the native library with cargo-ndk before `gradlew assembleRelease`. The steps are in `android/BUILD-WINDOWS.md`.

* A phone on the same Wi-Fi as the server timed out on the public address (NAT hairpinning). Cellular worked, through the relay. The phone now tries the server's LAN address and the public address together, on TCP 21200, and keeps whichever handshake finishes. Relay UDP uses that same IP, so the relay does not hairpin either. The LAN address comes from the account (`active_lan`), from a UDP 21202 announcement the server now broadcasts, and from the address that worked last time on this Wi-Fi or gateway. Windows clients on the LAN use the same race, the same announcement, and remember the winner for that network.
* Connect on Cabin Desk stayed on "PJDesk is still starting" because the release APK was built with `gradlew assembleRelease` alone, so `libpjdesk_mobile.so` was never in the APK. The release build now fails when `jniLibs/arm64-v8a/libpjdesk_mobile.so` (and the other ABIs) is missing. If that library is absent, the app says "Engine library missing from this build" instead of "still starting." Connect waits up to 15 seconds while the engine is actually starting, with a spinner, and if it fails it shows the reason. Settings → Diagnostics shows the engine, the server, the LAN address, the path that answered (or the raw timeout, refusal, or key mismatch), the relay, and the account.
* Home fits on one phone screen: account and status, Your ID with copy, the one-time password with show, copy, and refresh, a partner ID with Connect, and up to three recent computers. Sign out, the permanent password, and "When someone connects" are in Settings.
* Saved computers uses a Material 3 layout: a top app bar, a bottom bar with one icon and one label, an OS icon, a status dot, the ID and OS on their own lines, one Connect button, and Rename / Remove password / Remove in the overflow menu. Sort is Online first, Name, or Recently used. Filter is All, Online, or Offline. The choice is remembered on the phone.
* The app lock is a full-screen PIN pad with the logo, dot indicators, a numeric keypad, and fingerprint or face. A wrong PIN shakes and vibrates. Setting a PIN uses the same keypad, then confirm. The account password field is still there when saved passwords need it. Five wrong PINs still asks for the account password.
* The remote session is full screen. Touch mode taps to click, long-press and a two-finger tap right-click, and a drag drags. Trackpad mode moves a cursor, taps to click, scrolls with two fingers, and middle-clicks with three. Pinch zooms the picture, two fingers pan it, and a double-tap switches fit and 1:1. The picture follows the cursor when it is zoomed. A draggable bubble opens the toolbar: keyboard, Ctrl, Alt, Win, Tab, Esc, arrows, and F-keys, Ctrl+Alt+Del, input mode, display, quality, clipboard, and end.

## 0.6.11

Signing in on the phone now sets the PJDesk server the same way Windows does, and a phone that was already signed in fetches that address on startup and when Connect needs it. Saved computer passwords sync with the account, sealed so pjdesk.com only stores ciphertext. The phone stays signed in, and opening the app asks for a PIN or biometrics. Android, Windows, and the cloud worker are 0.6.11. Protocol stays 3. No tag, no release, and no GitHub Actions. If pjdesk.spader.us:21200 is already the active address, do not run the 0.6.1 slot-clearing SQL. Leave Require signed-in accounts off. Do not delete cloud-sync.json or cloud-state.json. The PJDesk Server account sign-in stays persistent. Install the server before the clients. On your PC, in cloud\, run `cloud\deploy.cmd` so migration 0013 (and 0008–0012 if they are not already applied) is applied before saved passwords can sync.

* Connect on a saved computer such as Cabin Desk was failing with "This phone has no server address yet" even though the account list was on the phone. Sign-in, startup, and Connect now take the active server address and key fingerprint from the account and give them to the engine. A dropped command is kept until the engine is up. Connect says "Getting your PJDesk server…" while it fetches that address.
* Saved computer passwords are sealed with a key derived from the account password (separate from the existing server-backup key). Windows and Android upload and download that blob. The server stores only the ciphertext. "Sign me out when I'm done" still wipes the visit and does not leave the vault key on that computer. The Saved header says the passwords sync, encrypted.
* The phone keeps the account refresh token in the Android Keystore, so the username and password are not asked again. Every time the app is opened, and after it has been in the background (1, 2, or 5 minutes), it asks for biometrics or the app PIN. The first sign-in asks for a 4 to 8 digit PIN and offers fingerprint or face. Five wrong PINs asks for the account password and then a new PIN. The saved-password key is only in memory while the app is unlocked. Incoming view access and hosting keep running while the app UI is locked.
* The account email and password fields are real text fields with username and password autofill hints, and a successful sign-in asks the password manager to save them. The website login already uses autocomplete username and current-password. The Windows sign-in fields have stable ids, and the Windows build exposes them through UI Automation. Show / Hide stays on those password fields.
* The release APK is signed on your PC with `C:\Users\BenSpader\.pjdesk-android\pjdesk-release.jks`, alias `pjdesk`. `android\sign-release.cmd` still asks for the passwords and does not save them.

## 0.6.10

Tapping Connect on a saved computer did nothing: no password, no "Connecting...", and no error. Connect now always answers. Android is 0.6.10. The workspace version is 0.6.10 with it. No Windows behavior change, no new migration, no tag, no release, and no GitHub Actions. If pjdesk.spader.us:21200 is already the active address, do not run the 0.6.1 slot-clearing SQL. Leave Require signed-in accounts off. Do not delete cloud-sync.json or cloud-state.json. The PJDesk Server account sign-in stays persistent. Install the server before the clients. On your PC, in cloud\, run `cloud\deploy.cmd` so migrations 0008–0012 are applied before relying on installer display names and before Delete from my account.

* **Connect** on Saved is a button of its own, not a tap on the whole card. A tap with no saved password opens the password dialog immediately. A saved password skips that and shows **Connecting to Cabin Desk…** on the Saved tab, then the remote screen. The same path is used from Home.
* A computer the account lists as offline says so, with **Try anyway**. A computer that does not answer says it is offline or did not answer. A wrong password opens the dialog again and says the password was wrong; a saved password that was wrong is cleared. Locked out shows the host's "try again in N minutes" text. If the relay cannot reach the computer, the dialog says it could not be reached, even through the relay. If this phone has no server address after a fresh install or sync, Connect says to sign in again or set the server. A replaced attempt does not flash "stopped".
* Each tap is written to the phone log (Settings → Share logs): the ID, the name, whether a password is saved, whether a server address is set, and the failure. The password itself is not logged.
* The release APK is signed on your PC with `C:\Users\BenSpader\.pjdesk-android\pjdesk-release.jks`, alias `pjdesk`. `android\sign-release.cmd` uses that path and alias unless `PJDESK_KEYSTORE` or `PJDESK_KEY_ALIAS` is set. It still asks for the passwords and does not save them. Gradle signs a release build only when `PJDESK_KEYSTORE`, `PJDESK_KEYSTORE_PASSWORD`, `PJDESK_KEY_ALIAS`, and `PJDESK_KEY_PASSWORD` are all set.

## 0.6.9

After a password was accepted, the phone could sit there with no screen capture and the Windows client only said that no picture arrived. The phone now says why, and asks for screen sharing if that consent is missing. The Android app lists the account's saved computers and can connect to them. Desktop, server, and Android are 0.6.9. No new migration, no tag, no release, and no GitHub Actions. If pjdesk.spader.us:21200 is already the active address, do not run the 0.6.1 slot-clearing SQL. Leave Require signed-in accounts off. Do not delete cloud-sync.json or cloud-state.json. The PJDesk Server account sign-in stays persistent. Install the server before the clients. On your PC, in cloud\, run `cloud\deploy.cmd` so migrations 0008–0012 are applied before relying on installer display names and before Delete from my account.

* A password that the phone accepts either starts the picture or tells Windows why it did not. If screen sharing is not already on, the phone posts a heads-up notification, **Allow screen sharing**, including when PJDesk is in the background. Windows says **Waiting for the phone to allow screen sharing** and waits up to 60 seconds. A decline or a missed consent says **screen sharing not allowed on the phone**, with the reason: tap the notification, MediaProjection consent missing or expired, or the foreground service is not running. The installed Windows 0.6.8 client still gives up after 15 seconds. This wait is in the 0.6.9 client. Request view access and the password path both wait for that consent. Android 14 and later still show the system dialog once per capture. A consent that is already running is reused. That system dialog cannot be pre-approved.
* Signed in, Saved on the phone is the account list: name, hostname, ID, last user, online, and OS, with search. **Connect** uses a password saved on this phone (Android Keystore). Recent is the computers this phone has connected to. Signed out, the list is only what is stored on the phone. Signing in adds this phone to the account, the same way Windows does.
* Connecting from the phone tries a direct path on the LAN first (UDP 21201), then the relay. The session line says over LAN or through the relay.
* The release build signs only when `PJDESK_KEYSTORE`, `PJDESK_KEYSTORE_PASSWORD`, `PJDESK_KEY_ALIAS`, and `PJDESK_KEY_PASSWORD` are all set. `android\sign-release.cmd` signs an unsigned APK on Windows with apksigner. It asks for the passwords and does not save them. The suggested keystore path is `C:\Users\BenSpader\AndroidStudioProjects\upload.jks`. That file is not confirmed to be the key that signed the installed app. A new key will not update the phone.

## 0.6.8

Check for updates now was refused at once with "That update request was not accepted," and the service log said nothing about it. The service also waited out its 30-minute timer before it could see a release that had just been published. The client installer no longer asks for a computer name, and it installs the background service on every install, including a silent update. Dropping files onto the viewer copies them to the other computer. On the computer being controlled, a small notice names who connected and then tucks into the corner. From Windows, Request view access asks the phone to allow the screen. Desktop, server, and Android are 0.6.8. No new migration, no tag, no release, and no GitHub Actions. If pjdesk.spader.us:21200 is already the active address, do not run the 0.6.1 slot-clearing SQL. Leave Require signed-in accounts off. Do not delete cloud-sync.json or cloud-state.json. The PJDesk Server account sign-in stays persistent. Install the server before the clients. On your PC, in cloud\, run `cloud\deploy.cmd` so migrations 0008–0012 are applied before relying on installer display names and before Delete from my account.

* **Check for updates now** talks to the PJDesk service after the request is read. A local update is not refused just because Windows could not impersonate the caller. A refusal is written to `C:\ProgramData\PJDesk\service\service.log` and shown in the window as the real reason. The automatic check still runs about every 30 minutes, asks GitHub again without a cached answer, and logs the latest tag. A check that is already running is logged and retried in about a minute.
* The client installer does not ask "Name this computer." A new install uses the Windows name. A name already saved on the computer is left as it is. Change it later in Saved → Rename, or in Settings. `/NAME=` is no longer a switch.
* Every client install, including a silent update, installs and starts the PJDesk service. There is no separate service choice. The desktop shortcut is still optional. Uninstall still removes the service.
* Dragging files or folders from Explorer onto the session window shows a copy cursor and sends them to the other computer's Desktop (or the receive folder set in Settings). The viewer says how many files it is sending, and the Files panel shows progress. The viewer opens without administrator rights so Explorer can drop onto it. Folders are copied as their files. To bring files back, use the Files panel: List their folder, then click a name. There is no drag from the remote desktop out to Explorer.
* When someone connects, the computer being controlled shows a small notice at the top-right of the primary screen. If they are signed in, the headline is the pjdesk.com account (for example ben@spader.us, or the account display name). The computer name is the line under that. If they are not signed in, the headline is the computer name, or the ID when the name is not known. The Windows user and the ID are shown with it. Disconnect ends the session. After about two seconds the notice shrinks to a small tab at the corner; click it to open it again. It does not take focus, and Windows is asked to leave it out of screen capture. It closes when the session ends. The same notice is used when the service is hosting in the signed-in session. On Android the existing notification names who connected.
* **Request view access**, on the Windows Connect page and on Saved → Menu, asks that phone to view its screen. No computer password is sent. Windows says **Waiting for &lt;phone&gt; to accept**. A decline says Declined. If nobody answers, it says Timed out. The phone needs 0.6.8.
* On the phone the prompt says **&lt;account&gt; wants to view your screen** when the Windows computer is signed in to pjdesk.com (ben@spader.us, or whatever account name was sent). Otherwise it uses the computer name, then the ID. Allow and Deny are on a heads-up notification and on a screen that can appear over the lock screen. Allow then starts Android's own screen-share consent.
* Settings → Trusted view access can **Always allow** an account (Always allow ben@spader.us) or a computer name or ID. **Use the permanent password** also skips the PJDesk prompt. Those requests connect without Allow / Deny. If the phone is already sharing, the existing capture keeps going. Android 14 and later give each consent one virtual display, so after Stop, or when nothing is sharing yet, Android asks to share the screen again. That system dialog cannot be pre-approved.

## 0.6.7

The 0.6.6 viewer panicked on every session as soon as the window opened: `there is no reactor running`. The window thread is not inside the Tokio runtime, and the session tasks used `tokio::spawn` from that thread. They now use the runtime handle captured when the session starts. "Opening the viewer window" is logged once. Desktop and server are 0.6.7. Android stays 0.6.7. No new migration, no tag, no release, and no GitHub Actions.

## 0.6.6

A password that CABINDESK accepted still did not open a session, and two computers on the same LAN never tried UDP 21201. Microsoft Defender was quarantining PJDesk as Trojan:Script/Wacatac.H!ml because Task Scheduler launched the updater. Desktop and server are 0.6.6. Android stays 0.6.7. There is no new cloud migration. If pjdesk.spader.us:21200 is already the active address, do not run the 0.6.1 slot-clearing SQL. Leave Require signed-in accounts off. Do not delete cloud-sync.json or cloud-state.json. The PJDesk Server account sign-in stays persistent. Install the server before the clients. No tag, no release, and no GitHub Actions.

* After the password is accepted, the viewer window is brought forward and each step is written to the log and to the status line. If the window or the picture does not start, the reason stays on the main window. On the computer that is sharing its screen, the log is `%APPDATA%\PJDesk\pjdesk.log`, or `C:\ProgramData\PJDesk\service\service.log` when the service is hosting.
* Two computers on the same LAN try UDP 21201 first (the host already listens there). The installer and the service add a firewall rule named PJDesk LAN for that port. New builds send one QUIC packet per datagram, so the relay is not handed a batch it would forward whole. A still screen sends a first picture instead of waiting for the desktop to change. Both computers need 0.6.6: a 0.6.5 host still sends the old batches, and its firewall may still drop UDP 21201.
* Installing the service when it is already installed updates it and starts it. It does not fail with "the specified service already exists."
* **Check for updates now** (Settings → General → Updates, Settings → About, and the tray menu) asks the PJDesk service to download and install. The window does not change permissions on `C:\ProgramData\PJDesk\updates`. If the service is not running, one administrator prompt runs the update. Automatic updates are the service, every 30 minutes, not a scheduled task. The "PJDesk Update" task is removed. People who had turned that task off will get automatic updates again until they uncheck the box.
* Installers are not packed. The executables carry version information (company, product, description, original filename) and an asInvoker manifest. `installer/build.sh` can Authenticode-sign with `PJDESK_SIGN_PFX` and `PJDESK_SIGN_PASSWORD` (osslsigncode or signtool). Signing is skipped when the certificate is not set.
* If Defender still quarantines a build, submit it as a false positive at https://www.microsoft.com/en-us/wdsi/filesubmission (Software developer, detection name Trojan:Script/Wacatac.H!ml). Send `pjdesk.exe`, the installer, and any quarantined file from `C:\ProgramData\PJDesk\updates`.

## 0.6.5

Saved → Menu has two different actions. **Remove from Saved** takes a computer off the list on this computer only. **Delete from my account** removes it from the pjdesk.com account, so it leaves Saved on every signed-in computer. Desktop and server are 0.6.5. Android stays 0.6.7. Run `cloud\deploy.cmd` from your PC so migration `0012_removed_computers.sql` is applied and the worker is deployed. Do that before using Delete from my account. If pjdesk.spader.us:21200 is already the active address, do not run the 0.6.1 slot-clearing SQL. Leave Require signed-in accounts off. Do not delete cloud-sync.json or cloud-state.json. The PJDesk Server account sign-in stays persistent. No tag, no release, and no GitHub Actions.

* **Remove from Saved** is on the row Menu. It asks before it removes the computer from Saved on this computer, and it clears the password saved here. The computer stays on the account and on your other computers.
* **Delete from my account** is on the same Menu. The confirm dialog names the computer and its ID. It is not offered for This computer. It deletes that computer's saved rows, its presence slot, and its sign-in from the account. Signed-in clients drop it on their next refresh (the open window checks about every 20 seconds). The password saved on the computer that clicked Delete is cleared. Signing in on that computer again adds it back.
* pjdesk.com → Account → My network → Computers lists the same computers, with **Delete from my account**.

## 0.6.4

A published 0.6.3 showed the Allow prompt before any password, so an unattended computer with a permanent password could not be reached. Desktop and server are 0.6.4. Android is 0.6.7. Install the server before the clients. If pjdesk.spader.us:21200 is already the active address, do not run the 0.6.1 slot-clearing SQL. Leave Require signed-in accounts off. Do not delete cloud-sync.json or cloud-state.json. On the server PC, in cloud\, run `cloud\deploy.cmd` so migrations 0008–0011 are applied before relying on installer display names. The PJDesk Server account sign-in stays persistent. No tag, no release, and no GitHub Actions.

* The host answers first with what it requires, and does not show Allow yet. The client uses the saved password or asks once. A valid permanent password starts the session with no on-device prompt when password access is allowed. Unattended mode (Android: Use the permanent password) does the same for any valid password once a permanent password is set. Allow is shown only when the mode still requires a person, when you choose Ask the person instead, or when the computer has no permanent password.
* A wrong permanent password is answered at once with Incorrect password for the ID. The password dialog opens again for a retype. A saved password that was wrong is removed, and the dialog says so. A right password never falls through to Allow.
* Five wrong passwords from one source, or twenty from any mix of sources, within about 20 minutes lock password checks for about 20 minutes. While locked, the host says locked out, try again in N minutes, and the client shows that with the time left. A correct password clears that source. The lock follows the address the relay saw, or the controller's name when no address is available. Lockouts are written to the host log.
* If nobody answers an Allow prompt, the client says No one at that computer answered the request. An explicit decline stays a decline.
* A server address that resolves to a private LAN address is labeled LAN.
* **Check for updates now** is in Settings → General → Updates, Settings → About, and the tray menu. It downloads the latest release, shows the progress, installs it, and opens PJDesk again. **Update all online computers** on the server tells every online client to do that immediately, including when automatic updates are off. Android Settings → About has **Check for updates**, which opens the latest APK. A server push opens that download on the phone too.

## 0.6.3

The PJDesk Server window stays usable after an update without Administrator rights. Signing in on a client adds that computer to the account, and the sign-in checkbox starts checked so the visit does not stay on someone else's computer. Desktop and server are 0.6.3. Android is 0.6.6. Run `cloud\deploy.cmd` from your PC so migration `0011_saved_display_name.sql` is applied and the worker is deployed. Do that before expecting the installer name to sync. No tag, no release, and no GitHub Actions.

* Signing in on a client adds that computer (its ID, hostname, the name from the installer or Settings, and the operating system) to Saved computers on the account. If it is already there, the nickname stays and the other details refresh. It stays on the account after you sign out. Remove it from Saved → Menu → Remove, or on pjdesk.com. A computer that was hidden is shown again when you sign in on it.
* The sign-in screen has **Sign me out when I'm done (use on someone else's computer)**, checked each time the form opens. Checked, the sign-in lasts until you close PJDesk, Windows locks or signs out or the PC shuts down, or PJDesk is idle (30 minutes, changeable in Settings, with a 60 second warning and **Stay signed in**). The refresh token stays in memory. Signing out removes the synced Saved list from that visit, passwords saved during the visit, Recent entries from the visit, and the account email. The computer stays in the account. Unchecked, sign-in still survives a restart and an update. The header says **Signed in until you close PJDesk** and shows **Sign out**.
* **Just add this computer to my account**, under Sign in, signs in, adds this computer, and signs out immediately.
* The PJDesk Server window's own account sign-in stays persistent. Android uses the same checkbox, checked by default, and signs out when the app is closed or after the same idle time. Android is 0.6.6.

* Saving settings, signing in and out of the pjdesk.com account, the network address, the public-IP watch, Require signed-in accounts, promote, demote, sync, check for updates, install an update, and restarting the service go through the running PJDesk service. The window does not need to be elevated for those.
* The service accepts a command when the caller is an administrator (including a normal Windows sign-in whose administrator rights are filtered by UAC) or the request carries a valid pjdesk.com access token for the same account the server is signed in to. Other local users are denied. Every command is written to the audit log. Passwords and tokens are not.
* Install, uninstall, and stop still ask Windows to approve that one action. Restart as administrator stays as a link.
* After an automatic update the window and tray open in the user session, as before. When the service answers, the window says **Managed through the PJDesk service.** instead of the administrator warning.
* Sign-in stays across the update. The saved account is not removed.
* A desktop 0.6.3 that already told the server its operating system is asked for the computer name. Android 0.6.3, and a 0.6.3 with no operating system stored, are not asked. Android 0.6.6 is asked.

## 0.6.2

Saved computers shows the real computer name, the last signed-in user, the operating system, and whether the computer is online. Desktop and server are 0.6.2. Android is 0.6.5. Run `cloud\deploy.cmd` from your PC so migrations `0008_presence.sql`, `0009_saved_order.sql`, and `0010_display_name.sql` are applied and the worker is deployed. Do that before expecting the Windows computers to appear. No tag, no release, and no GitHub Actions.

* Each computer reports its hostname (Windows `COMPUTERNAME`; Android device name, or the model if the device name is unavailable), its operating system, and the last interactive user. On Windows that is the person at the console (`DOMAIN\user`, or just the user). On Android it is the profile name when Android provides one. The name stays when nobody is signed in. It is stored for the account owner only. A connection to some other computer does not receive it.
* Saved computers, after you are signed in, includes every computer registered on your server, not only ones you saved by hand. A row that was saved with no name is filled in. The nickname stays the main line, with the real hostname under it. The User column is the last signed-in person. The System column is the operating system. Online and offline come from the server. This computer is marked **This computer** and Connect is not offered for it.
* The header shows the PJDesk account you are signed in as, with **Sign out**. **Sign in** is shown only when you are not signed in. Signing out leaves the server address, the saved list, and the per-computer passwords on this computer.
* Saved computers works when you are not signed in. Add, remove, nickname, star, and order stay on this computer, with the hint **Sign in at pjdesk.com to sync across your computers**. Signing in merges that list into the account (one row per computer; the account's nickname wins if both have one) and then keeps them in sync. Signing out keeps the list and stops syncing. A password for a computer never leaves this computer. Run `cloud\deploy.cmd` so `0009_saved_order.sql` is applied.
* The Connect window puts Control remote desktop and Your desktop on the left, and Saved, Recent, and On this network on the right. The Remote ID field keeps the caret on the digit you just typed. Connect asks for the computer password only after that computer answers, unless a password saved on this computer is used. The client installer is `PJDesk-Client-Setup-<version>.exe`, and the same bytes are also published as `PJDesk-Setup-<version>.exe` so an already-installed client can still update. The installer can name this computer (`/NAME=`). Android asks on first run and again in Settings. That name is what other signed-in computers show, unless someone types a nickname on their own screen. A blank name uses the computer's own name.
* Sign-in stays across an update. A refresh that fails because pjdesk.com could not be reached retries later. The computer signs out only when the account says the sign-in is no longer valid. The account in the app is the pjdesk.com account. Manage it at https://pjdesk.com/account.
* A 0.6.1 desktop and a 0.6.3 Android keep working. The server asks for the new report only from a build that understands it (desktop 0.6.2, Android 0.6.4 and later, including 0.6.5). Hostnames already stored on the server show up after the server and the worker are updated, before each computer is updated. The user and the operating system fill in as each computer is updated.

## 0.6.1

Fixes the 0.6.0 upgrade that turned **Require signed-in accounts** on, published both servers as standby, and emptied the active slot. Desktop and server are 0.6.1. Android stays 0.6.3. Run `cloud\deploy.cmd` from your PC (it applies `0007_reach.sql` and deploys the worker). Do not deploy from CI, and do not turn **Require signed-in accounts** on until every computer has signed in.

* Grace mode is the default again. A 0.5.2 upgrade, a new network, and a server that never had the switch turned on all leave **Require signed-in accounts** off. A 0.6.0 server that pulled "on" from the cloud turns it back off and pushes that off. The switch stays on only after you turn it on.
* The server that owns the cloud copy stays the active server even when no computer is online. An explicit standby stays standby. A server publishes its own address, fingerprint, LAN addresses, and instance id, and only when something changed. It does not clear the other slot just because both servers share a fingerprint, and it does not overwrite a slot held by a different machine unless you confirm.
* A hostname in the Public address field is never replaced by a raw IP. If the saved address is the raw IP that `pjdesk.spader.us` already points at, the server puts the hostname back. **Use this computer's current server** publishes the address the computer is already using, and the button shows that address.
* The server checks its public IP about every five minutes at `https://pjdesk.com/api/ip` (the caller's address, two matching reads before a change counts). It updates only the slot it owns. A hostname is left alone. The Server window and Account → My network show **Public IP** and the LAN addresses. A signed-in client refreshes the network on a failed reconnect so it follows a new address.
* A client on the same LAN as the server connects to the server's private address first (about 1.5 seconds) and falls back to the public address or hostname. It does that when its own public IP matches the server's published public IP, or when a short LAN probe succeeds. LAN and public are also tried together, and the first handshake that finishes is kept. Every path checks the server key fingerprint, so a LAN address cannot be spoofed. The status line shows **LAN** or **Internet**. Two computers on the same LAN prefer each other's private addresses the same way; the viewer title says which path connected.

### Recover BENLENOVODESK

Do this in order. Installing 0.6.1 on the server fixes grace mode and the active role. The slot flap stops only after `cloud\deploy.cmd`, because the worker that is live today still has the 0.6.0 publish rules.

1. Install **PJDesk Server 0.6.1** on BENLENOVODESK and restart the service. Leave **Require signed-in accounts** off. Do not delete `cloud-sync.json` or `cloud-state.json`. Computers can connect again after this step, still using the address they already have.
2. On your PC, in the `cloud` folder, run `cloud\deploy.cmd`. That applies `0007_reach.sql` and deploys the worker, including `GET /api/ip`. Do this before any SQL that names the new columns.
3. After that deploy, clear the bad slots and this account's change log. From the `cloud` folder:

```
npx wrangler d1 execute pjdesk-cloud --remote --command "UPDATE networks SET require_accounts = 0, active_address = '', active_fingerprint = '', standby_address = '', standby_fingerprint = '', active_lan = '', standby_lan = '', active_public_ip = '', standby_public_ip = '', active_instance = '', standby_instance = '', active_ip_checked_at = 0, standby_ip_checked_at = 0 WHERE owner_id = (SELECT id FROM accounts WHERE email = 'ben@spader.us'); DELETE FROM network_changes WHERE network_id IN (SELECT id FROM networks WHERE owner_id = (SELECT id FROM accounts WHERE email = 'ben@spader.us'));"
```

4. Restart BENLENOVODESK (or wait about 30 seconds). It publishes `pjdesk.spader.us:21200` into the empty active slot when that hostname is the public address, or when the saved address is the raw IP that name points at. If the Public address field still shows the raw IP, type `pjdesk.spader.us:21200` and click **Update My network**.
5. Install 0.6.1 on the standby (`68.234.75.68`) and restart it. It fills the standby slot. If the window asks to replace a slot held by the other machine, confirm once.
6. Install 0.6.1 on the clients. A client that already saved `204.248.122.44:21200` should be set back to `pjdesk.spader.us:21200`, or signed in again after the server publishes the hostname. Do not click the fallback button while that field shows the raw IP.

While 0.6.0 is still running, editing `network-access.json` is not enough: the next cloud pull turns **Require signed-in accounts** back on if the database still has it on. Stop that service, or install 0.6.1, before relying on the file.

## 0.6.0

* Connecting to a computer still asks for that computer's own permanent or one-time password. Signing in with the PJDesk account does not fill that in and does not skip it. The connect dialog says **Password for** the computer's name, with **Save password for this computer**. That saved password stays on the computer (Windows DPAPI, or the Android Keystore) and is not uploaded. The account sign-in is labeled for the network and its servers.
* One PJDesk account signs in the server. In the PJDesk Server window, **PJDesk account** is that login (for example ben@spader.us), with **Sign out** and **Switch account**. It is Cloud Sync and backup, the active or standby role, pairing with the other server, and registration on My network (address, fingerprint, and slot). Allowed accounts and **Require signed-in accounts** follow that account. A second server that signs in with the same account becomes the standby. No join code. There is no separate server password. Install, stop, and uninstall still need a Windows administrator, because that is an operating-system rule. Older local accounts in `accounts.json` keep working for an older client. A new server does not need any. The log used to say "User accounts"; it now says there are no local accounts, or lists the older ones as legacy.

### Servers already signed in

BENLENOVODESK is the active server and the other server is the standby. Both are already signed in to PJDesk Cloud as ben@spader.us. Updating PJDesk Server keeps `cloud-sync.json`, `cloud-state.json`, and the role. They stay signed in. Nothing has to be re-entered: not the PJDesk password, not a join code, and not the public address. After the update the window shows **Signed in as ben@spader.us**. The local accounts the log listed as "User accounts: 2" stay in `accounts.json` and still work. They are not the setup login.

* Signing in to PJDesk Server with the network owner's PJDesk account fills My network. The active server writes the active address and its key; the standby server writes the standby slot. The Server window has a Public address, guessed from the name computers already use when that name points at this server, or from the public address and port. A local network address is shown and is not published until you confirm it. A server that is not the owner, or whose key does not match the one already listed, does not replace it unless you confirm. The Account page lists those changes. Run `cloud\deploy.cmd` again so migration `0006_network_changes.sql` is applied.
* If you are signed in on a computer that already has a server and a pinned key, and My network has no server yet, the client offers **Use this computer's current server for my network**. That offer is only for the network owner. An empty network says: "Your network has no server yet. Sign in to PJDesk Server on your server computer, or set it on pjdesk.com → Account → My network."
* A fresh install has no server until you sign in with a PJDesk account. The account's My network (pjdesk.com → Account → My network) holds the active server address, its pinned key, a standby, and the accounts allowed to use it. Any signed-in account manages its own network there. The Admin page is only site-wide (plans, accounts, versions) and can show that network read-only. The client receives the address after sign-in. Passwords for remote computers stay on the device (Windows DPAPI, or the Android Keystore) and are not uploaded.
* The server can require a short-lived access token, signed by pjdesk.com, before registration, ID lookup, punch, or relay. Until **Require signed-in accounts** is turned on, computers that already registered keep working. The Server window lists computers that have not signed in. Revoking an account or a device cuts it off. A refused peer is told "Sign in required" and learns nothing else.
* Unattended hosts sign in once, or enroll with a one-time code from My network, and keep a refresh token so they stay registered after a reboot.
* A network key is a single encrypted string, `pjdesk-net1:…`, created on pjdesk.com → Account → My network or in the PJDesk Server window. It carries the server addresses, the pinned keys, and an enrollment secret. A passphrase-protected key needs a passphrase shared separately (Argon2id). A self-contained key can be opened by anyone who has it; the page says so. Each key can expire, can be limited to a number of uses or to one account, and can be revoked. The page that creates it shows the string once, with Copy and a QR code. Windows pastes it. Android can paste it or scan the QR code. The device then stays signed in after a restart. The string and the passphrase are not stored on pjdesk.com and are not written to the log.
* Settings → Network, on Windows and Android, has **Import server config** and **Export server config** on the ID/Relay Server row, in the same place as RustDesk. Import reads a network key from the clipboard and applies it. Export copies the key this computer was given. Pasting a `pjdesk-net1:` key anywhere on the Sign in screen offers to import it. A renamed installer `PJDesk-Setup-<version>--<blob>.exe` is applied on install when the file name fits. The string stays encrypted. It is not RustDesk's reversed base64 of the host and key, so the server address is not readable from the text. A real key is longer than Windows' 255-character file-name limit, so the rename often cannot be created; Import and `pjdesk --config <blob>` still apply it.
* The automatic updater installs a release only when its version is newer than the one already installed. The 0.5.2 installer that pjdesk.com still publishes is not installed over 0.6.0, on the client or the server, including a manual check.
* Saved computers shows the remote computer's hostname, reported when it comes online and again at the password handshake, and refreshes it on each connect. An optional nickname (Menu → Rename) is shown above that hostname; the hostname stays visible. The ID is only in the ID column. System is the operating system the host reported, with that system's icon, and says Unknown only before the first successful connect. The column headers line up with the rows, and On this network no longer shifts Connect. A nickname is stored with the account's saved computers (still no passwords). Run `cloud\deploy.cmd` again so the hostname column exists before those names sync.

### Rollout (do this in order)

1. On your PC, run `cloud\deploy.cmd`. That applies `cloud/migrations/0003_networks.sql`, `cloud/migrations/0004_invites.sql`, `cloud/migrations/0005_computer_hostname.sql`, and `cloud/migrations/0006_network_changes.sql`, then deploys the Worker. Do not deploy from CI. Leave **Require signed-in accounts** off. If you already deployed once for 0.6.0, run `cloud\deploy.cmd` again so the network-key table, the saved-computer hostname column, and the network change log exist.
2. Open pjdesk.com → Account → My network. Signing in on the server fills the address; you can still set the active server, its key fingerprint, and the standby here. You do not need to be a site admin. The signing key is created the first time that page (or a client) talks to the network API.
3. Update **PJDesk Server** to 0.6.0 before any client. It stays in grace mode. Existing computers keep connecting. Sign in once in the Server window with the PJDesk account. That is Cloud Sync, the role, and My network. BENLENOVODESK and the standby are already signed in as ben@spader.us, so this update does not ask for that password again. Confirm the address if the window says it is only a local one. The Server window shows which computers have not signed in.
4. On each computer, Windows and Android, sign in with the PJDesk account (or an invited account, or a one-time enrollment code for a computer with nobody at it). Confirm it disappears from "Not signed in yet".
5. Only then turn on **Require signed-in accounts**, in the Server window or on My network. After that, a peer without a valid token cannot register, look up an ID, or use the relay.
6. Do not put 0.6.0 clients on a 0.5.2 server first. A 0.6.0 client that sends the new sign-in message to an older server drops that connection and retries once without it. A 0.5.2 client never sends it and keeps working until you turn the switch on.
7. Publish the release yourself after the computers are signed in. This change does not tag or deploy a release.

## Unreleased

* Android 0.6.3. Save on the permanent password writes it to the Android Keystore and gives it to the running host immediately. A confirmation stays on screen. The password and the "when someone connects" choice are still there after the app restarts. The password is not written to the log or to the plain preference file.
* Android 0.6.2. When someone connects is a pair of radio choices, and Use the permanent password turns on only after a password is saved. That choice is stored and the host honors it. Home shows Set permanent password. After Allow, the phone waits for a fresh screen-share consent and keeps the capture service in the mediaProjection type before the virtual display is created on the main thread. The status line says Ready, Waiting for approval, Waiting for screen sharing, Being controlled by a name, or Ended with the reason. The accessibility service does not request the floating shortcut button. Passwords are not written to the log.
* Android 0.6.1. The debug build no longer installs a second launcher icon. Home and Settings have Set permanent password (confirm field, Show/Hide, at least 8 characters) and Accept with password without asking. A Windows connection asks for screen sharing when you tap Allow, keeps the session up after that consent, and shows the real reason if it ends. Logs are tag `PJDesk` and a file you can share from Settings. The Windows viewer writes the same failures to `%APPDATA%\PJDesk\pjdesk.log` and does not ask for the password again unless it was wrong.
* Android app (`android/`, version 0.6.1) is a host and a controller. It shares `pjdesk-proto` and the relay through `pjdesk-link` and `pjdesk-mobile`. Video stays H.264 Annex-B: MediaCodec encodes on Android, and the Windows client keeps decoding with OpenH264. Android hosts accept taps, swipes, pinch, Back, Home, Recents, volume, and typed text. The Windows viewer maps mouse and scroll onto those gestures when the remote reports `android`.

## 0.5.2

* The PJDesk Server window lists each computer's version. A client that connected before 0.5.2 shows "unknown (<0.5.2)". A computer behind the current release on pjdesk.com gets an "update available" badge next to Update. This server's version is shown on the service card. The other Cloud Sync server's version comes from that server's health report. The unelevated status pipe carries the same device list and the other server's version.
* pjdesk.com `/admin/` is a dashboard: Overview, Accounts, Servers & devices, Plans, and an audit log. Plans are Free and Cloud Sync (the site does not advertise other prices). Granting a plan records who, when, the old plan, and the new plan. An expiry turns Cloud Sync off the next time that account is used. Server versions are stored from the sync health report. Desktop client versions stay on the PJDesk Server window, because the cloud copy is sealed. Local user accounts in the PJDesk Server window are not pjdesk.com plans.
* The client has a Saved computers page: search by name, ID, or operating system, sort, favorites, and a menu to rename, transfer a file, favorite, forget a password, or remove. The host reports its operating system during the session. Older hosts show an unknown icon. Operating-system marks are simple drawn shapes (a four-pane window, a laptop, a terminal, a phone, a monitor with a question mark), and the name and ID columns are left-aligned. A password is saved only after the other computer accepts it, with Windows DPAPI for the current user. The Remember box starts checked only when that option is on in Settings.
* A remote session can switch displays without reconnecting. The host sends each display's name, size, position, and DPI, and sends the list again when a display is added or resized. When the remote has more than one display, a tab strip across the top of the session window lists each screen and All screens. The tab in use is highlighted, and one click switches immediately. Ctrl+Alt+1 through 9 does the same. Separate windows stays available as a button at the end of that strip. One screen captures only that display. All screens is one capture of the whole virtual desktop, so the pointer crosses from one display to the next. Separate windows opens one local window per display, titled with the computer and the screen number. Each window can be dragged onto any local monitor and has its own fullscreen (a button, and F11) on the monitor it is sitting on. The session toolbar (files, chat, keys, End) stays on one window; every screen window has a short bar to go fullscreen or close that screen. Closing a window stops that display's capture and leaves the session up. Closing the last window, or End, ends the session. A display unplugged on the host closes its window and shows a short notice. The last view mode, the last display, and the window positions are remembered per computer and put back when the same local monitors are still attached. Ctrl+Alt+1 through 9 focuses that screen (or switches to it in the other modes), and Ctrl+Alt+Left/Right cycles.

## 0.5.1

* The server window, opened without Administrator rights, no longer pretends the data folder is empty. The running service publishes the fingerprint, update setting, last check, cloud role, signed-in email, and generation on a local pipe the interactive user can read. Sections the pipe does not cover say "Locked — open as administrator to view" instead of a blank sign-in form. Restart as administrator is a button. Icons are drawn, and the window uses the same type and theme as the client.
* An update no longer leaves a computer offline. The installer still stops the service and closes the program so it can replace the exe. The new program then starts the service again, retrying for a few seconds, and starts PJDesk in the signed-in user's session. The server update does the same for its service and tray. A second check runs if the service exits immediately. The result is written to the update log. The installer does not touch the server key, accounts, cloud sign-in, or update settings.

## 0.5.0

* The client window is a new layout: a left rail for this computer (ID, one-time password, status) and a main column to connect, with Recent, Favorites, Address book, and Discovered lists. Settings is a centered page with a sidebar, a scrolling body, and a fixed footer, so labels and Save stay on screen. Light and dark themes follow Windows unless you pick one. Icons are drawn shapes, so copy, show, refresh, and the check mark are no longer empty boxes. The in-session toolbar floats, collapses, and shows a tooltip on each button.
* Opening Settings no longer waits on Windows. The scheduled-task query (`schtasks`) and the service-status query used to run on the UI thread inside the click, which is why the window sat there. Those reads now happen on a background thread. The window paints from the last answer and shows a spinner until it arrives.
* A permanent password is still at least 8 characters. An account password is still the server's rule: 12 characters with upper and lower case, a digit, and a symbol. The hints say which is which.
* Transfer file connects the same way as Connect and opens the viewer's file panel.

## 0.4.1

* The Cloud Sync panel's backup-server note, and the README section **Backup server & moving the server**, describe the current switch: sign in on another PJDesk Server with the same account (it becomes the standby), then choose **Make this the primary** there, or use the tray, or `pjdesk-server cloud promote`. Computers still reach whichever server the account's network names, so DNS and the port forward have to move if the new primary is on a different network.
* The installers read the workspace version from `Cargo.toml` when none is passed. `installer/build.sh` and both NSIS scripts no longer fall back to 0.2.0.
* Unused imports and unused code the compiler warned about are removed. What the programs do is unchanged.

## 0.4.0

Protocol 3. A 0.3.7 peer still connects for a classic desktop session (keyboard
and mouse). Account key derivation, device IDs, and the sealed cloud-sync
format are unchanged. The cloud bundle gained an `epoch` field that older
servers ignore. What you can click in 0.4.0, and what is only prepared on the
host, is listed in `docs/RUSTDESK-PARITY.md`.

### Session (protocol 3)

* The computer being controlled accepts the new messages when the other side
  speaks protocol 3: a permission grant, file transfer in both directions
  (resume, a folder you choose, nothing is executed), text and image clipboard,
  chat, system-audio loopback, monitor choice, quality with adaptive bitrate,
  a privacy screen, blocking local input, keyboard mode, restart, and TCP
  tunnels to ports you list. View-only and the server policy are ceilings.
  Someone at the computer is asked before a session starts, unless unattended
  access is allowed.
* The viewer toolbar (View, Keys, Session, Files, Chat, End) sends those
  controls. Files shows progress in both directions and resumes a partial
  download. Chat, clipboard text and image, audio, monitors, quality, the
  privacy screen, blocking the remote keyboard, and restart are on the bar or
  the side panel. Restart asks twice, then the viewer reconnects. A direct
  address and **Find on this network** are on the connect panel. The host's
  permission grant is what decides which of these actually run.
* Session recording writes an MP4 under `Videos\PJDesk`, using the size of the
  picture. The Files list is the host's receive folder, not the whole disk.

### Cloud Sync: make this server the primary

* On a standby, **Make this the primary** (Cloud Sync panel, tray, or
  `pjdesk-server cloud promote`). It works whether or not the current primary
  is online. Confirm explains that the other server becomes the standby.
* The primary can **Step down to standby** (`pjdesk-server cloud demote`).
* `--yes` skips the prompt. If the service is running, the command asks it to
  do the switch. If it is not, the command does the switch itself.
* Promote downloads the latest copy, applies it, adds one to a generation
  number stored in the sealed copy, and writes that copy with this server's
  id. Later uploads send `If-Match` (or `If-None-Match: *` when the object is
  missing) and are skipped when the cloud generation is newer or already
  belongs to another server. Two servers do not both write.
* The previous primary notices the newer generation on its next check, stops
  uploading, switches itself to standby, applies the cloud copy, and logs a
  notice. The Cloud Sync panel, tray tooltip, and `cloud-status.json` show
  the role, the generation, and which server is active.
* Copies from 0.3.7 have no generation. Until someone promotes, those servers
  keep the old "whoever has computers connected uploads" behavior.
* PJDesk Cloud honors `If-Match` / `If-None-Match: *` and still accepts an
  unconditional upload from an older server. It also stores a small health
  record (`/api/sync/health`) so a primary can see a standby's version.
  **Redeploy the website** (`cloud\deploy.cmd`) or account-mode fencing and
  the standby health report stay on the old Worker. An own-bucket (R2) server
  does not need that deploy: R2 already honors `If-Match`.

### PJDesk Server updates

* **Install server updates automatically** is off until you turn it on (server
  window, or `pjdesk-server update-task`). `update-task --disable` turns it off.
* `pjdesk-server update` checks and installs a newer release.
  `pjdesk-server update --check` only reports. The tray has **Check for updates now**.
* The check uses `https://pjdesk.com/api/release`, then GitHub if that is
  unreachable. Pre-releases are skipped. An older version is never installed.
* The installer is still verified with `SHA256SUMS` in a private directory,
  and hashed again immediately before it is launched. The server installer
  stops and restarts the service and does not delete the data folder. If the
  install fails after the service was stopped, the installer starts it again.
* While the setting is on, the service checks about once an hour (30–60
  minutes, different on each server). A standby may update first. A primary
  that can see a standby waits until that standby reports the new version and
  the current generation, or until 24 hours pass, so both do not restart
  together. An optional maintenance window defaults to any time.
* The window shows the last check, the last result, and an available version.
  A failed check or a failed launch leaves the service running.
* The installer no longer registers the old always-on "PJDesk Server Update"
  task. Upgrading removes that task. Turn updates on again if you want them.

### Accounts

* A new or changed server account password must be at least 12 characters, with
  an uppercase letter, a lowercase letter, a digit, and a symbol. It must not
  contain the username, start or end with a space, or match a short list of
  common passwords. The window shows a checklist and a strength meter while
  you type. An older weak password still signs in. The app then opens
  **Change your password** and shows the server's reason if the new one is refused.
* This does not apply to the PJDesk Cloud sign-in on the server (pjdesk.com
  checks that password). One-time host passwords are unchanged.

### Server window

* The tray starts when you sign in to Windows. That is on unless you turn it
  off. The installer writes the Run key and starts the icon, so it is there
  after install and after the next sign-in. A second launch signals the icon
  that is already running instead of adding another.
* The menu can open the window, start, stop, or restart the service (Windows
  asks for administrator), check for updates, open the log folder, promote,
  step down, sync, or exit. The tooltip names the service, how many computers
  are connected, the cloud role, and the update status. Closing or minimizing
  the window leaves the tray running.

## 0.3.7

Security fixes from a full review. The wire protocol is still version 2, and the
pjdesk.com account key derivation is unchanged, so existing 0.3.5 and 0.3.6
installs keep their IDs, accounts, and saved sign-ins. See `SECURITY-AUDIT.md`
for the findings and for the steps that have to be done by hand (redeploy the
website, keep publishing `SHA256SUMS`, re-seal an existing unattended service
password).

### Server and client

* The server data folder and the client service folder are limited to SYSTEM
  and Administrators. The signed-in console user can still read the service
  folder, so the window can show the one-time password. Other local accounts
  cannot read the server key, account tokens, address-book passwords, or the
  one-time password, and cannot drop a request file the service would obey.
* Request files only accept a device ID, `*` for "update all", or `now` for a
  sync or reload.
* Login tokens are stored as a SHA-256 hash. Old tokens are rewritten the next
  time the server starts. Clients still send the raw token.
* A wrong password, a missing account, and a disabled account all take the
  same time and return the same error. New passwords must be at least 8
  characters. Existing ones still work.
* Password checking no longer holds the lock that every other connection needs.
* A permanent password locks out new sessions for five minutes after five
  misses. The one-time password still rotates every three misses.
* Open connections, sessions, and relay traffic are capped so one address
  cannot fill the process or the relay.
* Automatic update requires `SHA256SUMS`, writes the installer into a private
  directory under a random name, and checks the bytes again before running
  them. A download URL that is not on GitHub is refused. The installers are
  still not code-signed.
* The unattended service password is sealed so the sign-in-screen helper can
  read it, and the settings window passes it through a file instead of the
  command line. An old user-scoped blob is re-sealed the next time someone
  who can open it runs the helper.
* Installers stop Windows from loading DLLs out of the folder they were
  launched from.
* Every password field in the client and server windows has a Show/Hide control.

### Website

* Sessions expire after 90 days idle or one year from creation. Sign-in
  attempts are limited per network as well as per email. Creating an account
  no longer says whether the address was already taken. Sync downloads are
  capped at 8 MB. Pages send a content security policy and the account and
  admin scripts are files, not inline.

## 0.2.0

The project is now **PJDesk** (formerly DeskLink), with a graphical app, a
background service, user accounts and Windows installers. Update the server and
all clients together — the signaling protocol changed (version 2).

### Apps

* **A real window.** Double-clicking `pjdesk.exe` opens a RustDesk-style
  window: your ID and password on the left, a box to connect to another
  computer on the right, your saved computers below, and a Settings window
  (General, Security, Network, Display, About). The old text mode is still
  there as `pjdesk console`.
* **Server management window.** Double-clicking `pjdesk-server.exe` opens a
  window with the service status, the server key fingerprint, user accounts
  (add/remove/disable) and the registered computers.
* **Viewer toolbar.** A control bar drops down at the top of a session: full
  screen, actual size, refresh, **Ctrl+Alt+Del**, block input, and disconnect.

### Background service

* **Reach this computer at the sign-in screen.** Install the service (from the
  installer, the Settings ▸ Security tab, or `pjdesk service install`) and
  PJDesk runs from boot. It launches a helper as SYSTEM into the active
  session and follows the input desktop, so you can connect before anyone
  signs in and through UAC prompts, including Ctrl+Alt+Del.

### Accounts

* **Sign in to your server.** Create accounts on the server
  (`pjdesk-server account add`, or the server window). Signing in from the app
  saves your login (encrypted with DPAPI) and syncs an **address book** of
  saved computers to every machine you use. Each computer can remember its
  password.

### Installers

* **One-click setup.** `PJDesk-Setup` and `PJDesk-Server-Setup` install to
  Program Files, add shortcuts and an uninstaller, and can set up the service.

### Security

* Account passwords are stored as Argon2id hashes; a saved login is a random
  token, revocable by changing the password. Login timing doesn't reveal
  whether an account exists. Saved passwords and tokens are encrypted at rest
  with DPAPI. The service folder is locked to SYSTEM and Administrators.

## 0.1.1

Fixes from a code review of 0.1.0. The wire protocol is unchanged, but update the server and all clients together.

### Security

* **One password guess per connection request.** A controller could previously open several QUIC connections within one request and try a password on each.
* **Pending requests are capped.** The host handles at most 3 unanswered requests at once, so strangers who know an ID can't keep it busy with requests they never finish.
* **Only one controller at a time.** If two controllers authenticate at the same moment, the second is turned away and the first keeps streaming.
* **Separate UDP tokens for host and controller,** so neither side can bind to the relay as the other.
* **Relay routes can't be hijacked.** A bind from an address that is actively relaying another session is ignored.
* **Server data folder locked down.** The service limits `C:\ProgramData\PJDesk\server` (private key, ID registry and log) to SYSTEM and Administrators when it is installed and each time it starts. On Linux the private key file is created with mode 0600.

### Host

* **DXGI capture recovers by itself.** It follows resolution changes, treats a temporarily unavailable desktop (UAC prompt, lock screen, session switch) as a pause instead of an error, and moves back from the GDI fallback to DXGI when it becomes available again.
* **GDI capture** follows resolution and monitor-layout changes, and no longer makes the host's mouse cursor flicker.
* **Keyboard and mouse:** keys and buttons are tracked by what Windows actually accepted, so "release everything" can't leave a key stuck. Pause, NumLock and PrintScreen, whose scan codes don't survive `SendInput`, are sent as virtual keys. Numpad digits arrive as digits whatever the host's NumLock state.
* **Older Windows:** the per-monitor DPI API is looked up at run time, so `pjdesk.exe` also starts on Windows 10 1607 and Windows Server 2016, where it falls back to the Windows 8.1 DPI API.

### Server

* **The Windows service** closes its sockets before reporting "stopped", so a restart can bind right away. It reports an error code to Windows when the server fails, and a relative `--data-dir` is resolved at install time.

### Viewer

* Mouse positions are exact to the pixel at the right and bottom edges.
* Keyframe requests after a decoding error are rate-limited, and every frame is acknowledged even if it fails to decode, so the stream can't stall.
* `pjdesk connect 123 456 789` accepts the ID with spaces, as the host displays it.

### Docs

* Commands use the `.\` prefix, so they work in PowerShell as well as Command Prompt.

## 0.1.0

First release: signaling server with ID registry and UDP relay (console or Windows service), and one client that both shares and controls, with P2P over QUIC, SPAKE2 passwords, DXGI/GDI capture, OpenH264 streaming and SendInput control.
