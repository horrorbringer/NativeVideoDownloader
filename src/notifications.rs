use std::sync::atomic::{AtomicBool, Ordering};
use tracing::info;

static NOTIFICATIONS_ENABLED: AtomicBool = AtomicBool::new(true);
static SOUND_ENABLED: AtomicBool = AtomicBool::new(true);

/// Sets whether desktop system notifications are enabled
pub fn set_notifications_enabled(enabled: bool) {
    NOTIFICATIONS_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Checks whether desktop notifications are enabled
pub fn is_notifications_enabled() -> bool {
    NOTIFICATIONS_ENABLED.load(Ordering::Relaxed)
}

/// Sets whether audio chimes accompany desktop notifications
pub fn set_sound_enabled(enabled: bool) {
    SOUND_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Checks whether notification sound is enabled
pub fn is_sound_enabled() -> bool {
    SOUND_ENABLED.load(Ordering::Relaxed)
}

/// Sends a native desktop system notification with optional audio chime
pub fn send_notification(title: &str, subtitle: &str, message: &str, is_error: bool) {
    if !is_notifications_enabled() {
        return;
    }
    dispatch_system_notification(title, subtitle, message, is_error, is_sound_enabled());
}

/// Sends an explicit test notification regardless of notification toggle state
pub fn send_test_notification() {
    let sound = is_sound_enabled();
    let subtitle = if sound {
        "Test Alert (Sound Active)"
    } else {
        "Test Alert (Muted)"
    };
    dispatch_system_notification(
        "Native Video Downloader",
        subtitle,
        "Desktop notifications and audio alerts are functioning perfectly!",
        false,
        sound,
    );
}

#[cfg(not(target_os = "macos"))]
fn get_app_icon_temp_path() -> Option<std::path::PathBuf> {
    let temp_icon = std::env::temp_dir().join("native_video_downloader_logo.png");
    if !temp_icon.exists() {
        const ICON_BYTES: &[u8] = include_bytes!("../assets/app_icon.png");
        let _ = std::fs::write(&temp_icon, ICON_BYTES);
    }
    if temp_icon.exists() {
        Some(temp_icon)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn ensure_macos_bundle_registered() {
    static REGISTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if REGISTERED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    let candidates = [
        std::path::PathBuf::from("dist/Native Video Downloader.app"),
        std::path::PathBuf::from("/Applications/Native Video Downloader.app"),
    ];

    for candidate in candidates {
        if candidate.exists() && candidate.join("Contents/Info.plist").exists() {
            let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
            let _ = std::process::Command::new(lsregister)
                .args(["-f", &candidate.to_string_lossy()])
                .status();
            break;
        }
    }
}

fn dispatch_system_notification(
    title: &str,
    subtitle: &str,
    message: &str,
    is_error: bool,
    sound_on: bool,
) {
    info!("Dispatching desktop notification: [{}] {}", title, message);
    let title = title.to_string();
    let subtitle = subtitle.to_string();
    // Cleanly truncate message if too long so notification card remains concise
    let mut message = message.trim().to_string();
    if message.chars().count() > 180 {
        message = format!("{}...", message.chars().take(177).collect::<String>());
    }

    tokio::task::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        {
            ensure_macos_bundle_registered();

            let sound_clause = if sound_on {
                let sound = if is_error { "Basso" } else { "Glass" };
                format!(" sound name \"{}\"", sound)
            } else {
                String::new()
            };

            let formatted_subtitle = if is_error {
                format!("⚠️ {}", subtitle)
            } else {
                format!("✅ {}", subtitle)
            };

            // Escape double quotes and backslashes for AppleScript string literals
            let safe_title = title.replace('\\', "\\\\").replace('"', "\\\"");
            let safe_subtitle = formatted_subtitle.replace('\\', "\\\\").replace('"', "\\\"");
            let safe_msg = message.replace('\\', "\\\\").replace('"', "\\\"");

            // Route through registered bundle ID so macOS attaches the AppIcon logo
            let script = format!(
                "try\n\
                     tell application id \"com.native.videodownloader\" to display notification \"{}\" with title \"{}\" subtitle \"{}\"{}\n\
                 on error\n\
                     display notification \"{}\" with title \"{}\" subtitle \"{}\"{}\n\
                 end try",
                safe_msg, safe_title, safe_subtitle, sound_clause,
                safe_msg, safe_title, safe_subtitle, sound_clause
            );

            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(script)
                .spawn();
        }

        #[cfg(target_os = "windows")]
        {
            let audio_tag = if sound_on {
                ""
            } else {
                "<audio silent=\"true\"/>"
            };

            let icon_xml = if let Some(icon_path) = get_app_icon_temp_path() {
                format!(
                    "<image placement=\"appLogoOverride\" hint-crop=\"circle\" src=\"{}\"/>",
                    icon_path.to_string_lossy().replace('\\', "/")
                )
            } else {
                String::new()
            };

            // Windows PowerShell Toast notification with application logo
            let script = format!(
                "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
                 [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null; \
                 $template = '<toast><visual><binding template=\"ToastGeneric\">{}<text>{}</text><text>{}</text></binding></visual>{}</toast>'; \
                 $xml = [Windows.Data.Xml.Dom.XmlDocument]::new(); \
                 $xml.LoadXml($template); \
                 $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
                 [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Native Video Downloader').Show($toast);",
                icon_xml,
                title.replace('\'', "''"),
                message.replace('\'', "''"),
                audio_tag
            );

            let _ = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &script])
                .spawn();
        }

        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            let urgency = if is_error { "critical" } else { "normal" };
            let mut cmd = std::process::Command::new("notify-send");
            cmd.arg("-u").arg(urgency);
            if let Some(icon_path) = get_app_icon_temp_path() {
                cmd.arg("-i").arg(icon_path);
            }
            cmd.arg(&title).arg(&message);
            let _ = cmd.spawn();
        }
    });
}

