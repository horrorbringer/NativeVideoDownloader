/// Supported application theme modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Light = 0,
    Dark = 1,
    Auto = 2,
}

impl ThemeMode {
    pub fn from_i32(val: i32) -> Self {
        match val {
            0 => Self::Light,
            1 => Self::Dark,
            _ => Self::Auto,
        }
    }

    pub fn to_i32(self) -> i32 {
        self as i32
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Auto => "auto",
        }
    }
}

impl Default for ThemeMode {
    fn default() -> Self {
        Self::Auto
    }
}

impl std::str::FromStr for ThemeMode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mode = match s.trim().to_lowercase().as_str() {
            "0" | "light" => Self::Light,
            "1" | "dark" => Self::Dark,
            _ => Self::Auto,
        };
        Ok(mode)
    }
}

/// Detects the current OS system appearance (true = Dark, false = Light)
/// with cross-platform support across macOS, Windows, and Linux.
pub fn is_system_dark_mode() -> bool {
    // 1. Primary cross-platform detection via dark-light crate
    // Uses macOS NSAppearance, Windows Registry, Linux XDG Desktop Portal / FreeDesktop D-Bus
    if let Ok(mode) = dark_light::detect() {
        match mode {
            dark_light::Mode::Dark => return true,
            dark_light::Mode::Light => return false,
            dark_light::Mode::Unspecified => {}
        }
    }

    // 2. Platform-specific fallback mechanisms:
    #[cfg(target_os = "macos")]
    {
        // On macOS: `defaults read -g AppleInterfaceStyle`
        // Returns "Dark" if dark mode is active.
        // Returns non-zero exit status if in Light mode (key does not exist).
        if let Ok(output) = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
        {
            if output.status.success() {
                let style = String::from_utf8_lossy(&output.stdout).trim().to_lowercase();
                if style.contains("dark") {
                    return true;
                }
            } else {
                // Key does not exist in standard Light mode
                return false;
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows: query Personalize registry AppsUseLightTheme
        // 0 = Dark mode, 1 = Light mode
        if let Ok(output) = std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if text.contains("0x0") {
                    return true;
                } else if text.contains("0x1") {
                    return false;
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // On Linux GNOME/FreeDesktop: query color-scheme
        if let Ok(output) = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
        {
            if output.status.success() {
                let scheme = String::from_utf8_lossy(&output.stdout).trim().to_lowercase();
                if scheme.contains("dark") {
                    return true;
                } else if scheme.contains("light") || scheme.contains("default") {
                    return false;
                }
            }
        }
    }

    // Default safe fallback if detection is inconclusive
    true
}

/// Resolves whether the application should be displayed in dark palette
/// according to the selected theme mode and the system appearance.
pub fn resolve_is_dark(mode: ThemeMode) -> bool {
    match mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::Auto => is_system_dark_mode(),
    }
}

/// Synchronizes the native operating system window appearance (macOS Cocoa title bar)
/// to match the application's active theme.
#[cfg(target_os = "macos")]
pub fn sync_macos_app_appearance(mode: ThemeMode) {
    use std::ffi::{c_char, c_void};

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    type MsgSendNoArgs = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendOneArg = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendStr = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char) -> *mut c_void;

    unsafe {
        let msg_send_no_args: MsgSendNoArgs = std::mem::transmute(objc_msgSend as *const ());
        let msg_send_one_arg: MsgSendOneArg = std::mem::transmute(objc_msgSend as *const ());
        let msg_send_str: MsgSendStr = std::mem::transmute(objc_msgSend as *const ());

        let ns_app_class = objc_getClass(b"NSApplication\0".as_ptr() as *const c_char);
        if ns_app_class.is_null() {
            return;
        }
        let shared_app_sel = sel_registerName(b"sharedApplication\0".as_ptr() as *const c_char);
        let app = msg_send_no_args(ns_app_class, shared_app_sel);
        if app.is_null() {
            return;
        }

        let set_appearance_sel = sel_registerName(b"setAppearance:\0".as_ptr() as *const c_char);

        let appearance_obj = match mode {
            ThemeMode::Auto => std::ptr::null_mut(),
            ThemeMode::Light => {
                let ns_appearance_class = objc_getClass(b"NSAppearance\0".as_ptr() as *const c_char);
                let app_named_sel = sel_registerName(b"appearanceNamed:\0".as_ptr() as *const c_char);
                let ns_string_class = objc_getClass(b"NSString\0".as_ptr() as *const c_char);
                let str_utf8_sel = sel_registerName(b"stringWithUTF8String:\0".as_ptr() as *const c_char);

                let aqua_str = msg_send_str(ns_string_class, str_utf8_sel, b"NSAppearanceNameAqua\0".as_ptr() as *const c_char);
                msg_send_one_arg(ns_appearance_class, app_named_sel, aqua_str)
            }
            ThemeMode::Dark => {
                let ns_appearance_class = objc_getClass(b"NSAppearance\0".as_ptr() as *const c_char);
                let app_named_sel = sel_registerName(b"appearanceNamed:\0".as_ptr() as *const c_char);
                let ns_string_class = objc_getClass(b"NSString\0".as_ptr() as *const c_char);
                let str_utf8_sel = sel_registerName(b"stringWithUTF8String:\0".as_ptr() as *const c_char);

                let dark_aqua_str = msg_send_str(ns_string_class, str_utf8_sel, b"NSAppearanceNameDarkAqua\0".as_ptr() as *const c_char);
                msg_send_one_arg(ns_appearance_class, app_named_sel, dark_aqua_str)
            }
        };

        msg_send_one_arg(app, set_appearance_sel, appearance_obj);
    }
}

/// Sets the macOS application and Dock icon dynamically at runtime using the embedded logo.
#[cfg(target_os = "macos")]
pub fn set_macos_app_icon() {
    use std::ffi::{c_char, c_void};

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    type MsgSendNoArgs = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendOneArg = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendData = unsafe extern "C" fn(*mut c_void, *mut c_void, *const u8, usize) -> *mut c_void;

    unsafe {
        let msg_send_no_args: MsgSendNoArgs = std::mem::transmute(objc_msgSend as *const ());
        let msg_send_one_arg: MsgSendOneArg = std::mem::transmute(objc_msgSend as *const ());
        let msg_send_data: MsgSendData = std::mem::transmute(objc_msgSend as *const ());

        let ns_app_class = objc_getClass(b"NSApplication\0".as_ptr() as *const c_char);
        if ns_app_class.is_null() {
            return;
        }
        let shared_app_sel = sel_registerName(b"sharedApplication\0".as_ptr() as *const c_char);
        let app = msg_send_no_args(ns_app_class, shared_app_sel);
        if app.is_null() {
            return;
        }

        const ICON_BYTES: &[u8] = include_bytes!("../assets/app_icon.png");

        let ns_data_class = objc_getClass(b"NSData\0".as_ptr() as *const c_char);
        let data_with_bytes_sel = sel_registerName(b"dataWithBytes:length:\0".as_ptr() as *const c_char);
        let data_obj = msg_send_data(ns_data_class, data_with_bytes_sel, ICON_BYTES.as_ptr(), ICON_BYTES.len());
        if data_obj.is_null() {
            return;
        }

        let ns_image_class = objc_getClass(b"NSImage\0".as_ptr() as *const c_char);
        let alloc_sel = sel_registerName(b"alloc\0".as_ptr() as *const c_char);
        let init_with_data_sel = sel_registerName(b"initWithData:\0".as_ptr() as *const c_char);
        let image_alloc = msg_send_no_args(ns_image_class, alloc_sel);
        let image_obj = msg_send_one_arg(image_alloc, init_with_data_sel, data_obj);
        if image_obj.is_null() {
            return;
        }

        let set_app_icon_sel = sel_registerName(b"setApplicationIconImage:\0".as_ptr() as *const c_char);
        msg_send_one_arg(app, set_app_icon_sel, image_obj);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn sync_macos_app_appearance(_mode: ThemeMode) {}

#[cfg(not(target_os = "macos"))]
pub fn set_macos_app_icon() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_mode_conversions() {
        assert_eq!(ThemeMode::from_i32(0), ThemeMode::Light);
        assert_eq!(ThemeMode::from_i32(1), ThemeMode::Dark);
        assert_eq!(ThemeMode::from_i32(2), ThemeMode::Auto);
        assert_eq!(ThemeMode::from_i32(99), ThemeMode::Auto);

        assert_eq!(ThemeMode::Light.to_i32(), 0);
        assert_eq!(ThemeMode::Dark.to_i32(), 1);
        assert_eq!(ThemeMode::Auto.to_i32(), 2);

        use std::str::FromStr;
        assert_eq!(ThemeMode::from_str("0").unwrap(), ThemeMode::Light);
        assert_eq!(ThemeMode::from_str("light").unwrap(), ThemeMode::Light);
        assert_eq!(ThemeMode::from_str("1").unwrap(), ThemeMode::Dark);
        assert_eq!(ThemeMode::from_str("dark").unwrap(), ThemeMode::Dark);
        assert_eq!(ThemeMode::from_str("2").unwrap(), ThemeMode::Auto);
        assert_eq!(ThemeMode::from_str("auto").unwrap(), ThemeMode::Auto);
        assert_eq!(ThemeMode::from_str("system").unwrap(), ThemeMode::Auto);

        assert_eq!(ThemeMode::Light.as_str(), "light");
        assert_eq!(ThemeMode::Dark.as_str(), "dark");
        assert_eq!(ThemeMode::Auto.as_str(), "auto");
    }

    #[test]
    fn test_resolve_is_dark() {
        assert!(!resolve_is_dark(ThemeMode::Light));
        assert!(resolve_is_dark(ThemeMode::Dark));
        // Auto should return a valid boolean without crashing
        let _ = resolve_is_dark(ThemeMode::Auto);
    }

    #[test]
    fn test_is_system_dark_mode_runs_safely() {
        let is_dark = is_system_dark_mode();
        println!("Detected system dark mode: {}", is_dark);
    }
}
