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

            let script = format!(
                "display notification \"{}\" with title \"{}\" subtitle \"{}\"{}",
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

            // Windows PowerShell Toast notification
            let script = format!(
                "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
                 [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null; \
                 $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
                 $text = $xml.GetElementsByTagName('text'); \
                 $text[0].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
                 $text[1].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
                 $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
                 [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Native Video Downloader').Show($toast);",
                title.replace('\'', "''"), message.replace('\'', "''")
            );

            let _ = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &script])
                .spawn();
        }

        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            let urgency = if is_error { "critical" } else { "normal" };
            let _ = std::process::Command::new("notify-send")
                .arg("-u")
                .arg(urgency)
                .arg(&title)
                .arg(&message)
                .spawn();
        }
    });
}

