//! The one place that knows where a Minecraft install lives on each OS.
//!
//! Resolution order everywhere: explicit flag (handled by the caller) > environment variable >
//! project-local copy (`vanilla/client.jar`) > the launcher's install under [`minecraft_dir`].
//!
//! Env overrides: `BASALT_MINECRAFT_DIR` (launcher dir), `BASALT_JAR`, `BASALT_ASSETS`,
//! `BASALT_ASSET_INDEX`.
use std::path::{Path, PathBuf};

/// The game version Basalt targets (launcher `versions/<VERSION>/`).
pub const MC_VERSION: &str = "26.2";

/// The user's home directory (`HOME`, or `USERPROFILE` on Windows). Falls back to ".".
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The vanilla launcher's game directory for the given OS name (`std::env::consts::OS` values).
/// Pure function so it can be unit-tested for every OS from any host.
pub fn minecraft_dir_for(os: &str, home: &Path, appdata: Option<&Path>) -> PathBuf {
    match os {
        "macos" => home.join("Library/Application Support/minecraft"),
        "windows" => appdata
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join("AppData/Roaming"))
            .join(".minecraft"),
        _ => home.join(".minecraft"),
    }
}

/// The launcher's game directory on this machine (honours `BASALT_MINECRAFT_DIR`).
pub fn minecraft_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("BASALT_MINECRAFT_DIR").filter(|v| !v.is_empty()) {
        return d.into();
    }
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    minecraft_dir_for(std::env::consts::OS, &home_dir(), appdata.as_deref())
}

/// Client jar to read textures/models from: `BASALT_JAR`, else `vanilla/client.jar` if it exists,
/// else `<mc>/versions/26.2/26.2.jar` if it exists, else `vanilla/client.jar` (so error messages
/// name the conventional location).
pub fn client_jar() -> PathBuf {
    if let Some(j) = std::env::var_os("BASALT_JAR").filter(|v| !v.is_empty()) {
        return j.into();
    }
    let local = PathBuf::from("vanilla").join("client.jar");
    if local.exists() {
        return local;
    }
    let launcher = minecraft_dir()
        .join("versions")
        .join(MC_VERSION)
        .join(format!("{MC_VERSION}.jar"));
    if launcher.exists() {
        return launcher;
    }
    local
}

/// Launcher asset store directory (`indexes/` + `objects/`): `BASALT_ASSETS` or `<mc>/assets`.
pub fn assets_dir() -> PathBuf {
    std::env::var_os("BASALT_ASSETS")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| minecraft_dir().join("assets"))
}

/// Asset index id: `BASALT_ASSET_INDEX`, else read from `<mc>/versions/26.2/26.2.json`.
pub fn asset_index_id() -> Option<String> {
    if let Some(i) = std::env::var("BASALT_ASSET_INDEX")
        .ok()
        .filter(|v| !v.is_empty())
    {
        return Some(i);
    }
    let p = minecraft_dir()
        .join("versions")
        .join(MC_VERSION)
        .join(format!("{MC_VERSION}.json"));
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
    v["assetIndex"]["id"].as_str().map(String::from)
}

/// (assets dir, asset index id) as used by the client's sound/panorama/language loaders.
pub fn default_assets() -> (PathBuf, Option<String>) {
    (assets_dir(), asset_index_id())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn per_os_layout() {
        let h = Path::new("/home/u");
        assert_eq!(
            minecraft_dir_for("macos", h, None),
            Path::new("/home/u/Library/Application Support/minecraft")
        );
        assert_eq!(
            minecraft_dir_for("linux", h, None),
            Path::new("/home/u/.minecraft")
        );
        assert_eq!(
            minecraft_dir_for("windows", h, Some(Path::new("C:/Users/u/AppData/Roaming"))),
            Path::new("C:/Users/u/AppData/Roaming/.minecraft")
        );
        assert_eq!(
            minecraft_dir_for("windows", h, None),
            Path::new("/home/u/AppData/Roaming/.minecraft")
        );
    }
}
