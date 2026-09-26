use tracing::info;

/// Sends a native desktop system notification with audio chime
pub fn send_notification(title: &str, subtitle: &str, message: &str, is_error: bool) {
    info!("Dispatching desktop notification: [{}] {}", title, message);
    let title = title.to_string();
    let subtitle = subtitle.to_string();
    let message = message.to_string();

    tokio::task::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        {
            let sound = if is_error { "Basso" } else { "Glass" };
            // Escape double quotes and backslashes for AppleScript string literals
            let safe_title = title.replace('\\', "\\\\").replace('"', "\\\"");
            let safe_subtitle = subtitle.replace('\\', "\\\\").replace('"', "\\\"");
            let safe_msg = message.replace('\\', "\\\\").replace('"', "\\\"");

            let script = format!(
                "display notification \"{}\" with title \"{}\" subtitle \"{}\" sound name \"{}\"",
                safe_msg, safe_title, safe_subtitle, sound
            );

            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(script)
                .spawn();
        }

        #[cfg(target_os = "windows")]
        {
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
