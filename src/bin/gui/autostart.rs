//! Start with the session: XDG autostart on Linux, the `Run` key on Windows.

#[cfg(target_os = "linux")]
pub fn set(enabled: bool) -> std::io::Result<()> {
    let dir = dirs::config_dir().ok_or_else(|| std::io::Error::other("no config dir"))?.join("autostart");
    let file = dir.join("bticino-easykit.desktop");
    if !enabled {
        return match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(&dir)?;
    let exe = std::env::current_exe()?;
    std::fs::write(
        file,
        format!(
            "[Desktop Entry]\nType=Application\nName=BTicino EASYKIT\nExec=\"{}\" --minimized\nIcon=bticino-easykit\nX-GNOME-Autostart-enabled=true\n",
            exe.display()
        ),
    )
}

#[cfg(windows)]
pub fn set(enabled: bool) -> std::io::Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    let run = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Run", KEY_SET_VALUE)?;
    if enabled {
        let exe = std::env::current_exe()?;
        run.set_value("BTicinoEasykit", &format!("\"{}\" --minimized", exe.display()))
    } else {
        match run.delete_value("BTicinoEasykit") {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn set(_enabled: bool) -> std::io::Result<()> {
    Err(std::io::Error::other("autostart is not supported on this system"))
}
