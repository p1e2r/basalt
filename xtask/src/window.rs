//! Capture only the Minecraft window (never the full screen).
//!
//! - macOS: tools/mc-window-shot.swift / mc-window-bounds.swift compiled once with `swiftc` (cached in
//!   target/xtask/), which find the window by title prefix via CoreGraphics and call `screencapture -l`.
//! - Windows: a PowerShell script (written to target/xtask/) finds the window by title prefix with
//!   user32 and renders it with PrintWindow (PW_RENDERFULLCONTENT).
//! - Linux/X11: `xdotool` finds the window; `import -window ID` (ImageMagick) or `xwd -id ID | convert`
//!   captures it. Wayland-only sessions are not supported (a clear error is printed).
//!
//! The window title prefix is "Minecraft" (override with MC_WINDOW_TITLE).
use anyhow::{bail, Context, Result};
use std::process::Command;

#[allow(dead_code)]
fn title() -> String {
    std::env::var("MC_WINDOW_TITLE")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "Minecraft".into())
}

#[allow(dead_code)]
fn have(tool: &str, probe: &str) -> bool {
    Command::new(tool).arg(probe).output().is_ok()
}

/// Compile a swift tool once into target/xtask/<name> (rebuilt when the source is newer).
#[cfg(target_os = "macos")]
fn swift_tool(name: &str) -> Result<std::path::PathBuf> {
    use std::path::PathBuf;
    let src = PathBuf::from(format!("tools/{name}.swift"));
    let bin = PathBuf::from("target/xtask").join(name);
    let stale = match (
        std::fs::metadata(&bin).and_then(|m| m.modified()),
        std::fs::metadata(&src).and_then(|m| m.modified()),
    ) {
        (Ok(b), Ok(s)) => s > b,
        _ => true,
    };
    if stale {
        std::fs::create_dir_all("target/xtask")?;
        let st = crate::util::status(
            Command::new("swiftc")
                .arg("-O")
                .arg(&src)
                .arg("-o")
                .arg(&bin),
        )
        .context("swiftc not found: install the Xcode command line tools")?;
        if !st.success() {
            bail!("swiftc failed for {}", src.display());
        }
    }
    Ok(bin)
}

#[allow(dead_code)]
const WIN_PS1: &str = r#"param([string]$Out, [string]$Title, [string]$Mode)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices; using System.Text;
public class W {
  public delegate bool EP(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EP cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint f);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static IntPtr Find(string prefix) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      if (!IsWindowVisible(h)) return true;
      StringBuilder sb = new StringBuilder(256); GetWindowText(h, sb, 256);
      if (sb.ToString().StartsWith(prefix)) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@
[W]::SetProcessDPIAware() | Out-Null
$h = [W]::Find($Title)
if ($h -eq [IntPtr]::Zero) { [Console]::Error.WriteLine("no window titled '$Title*'"); exit 1 }
$r = New-Object W+RECT
[W]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.R - $r.L; $ht = $r.B - $r.T
if ($Mode -eq 'bounds') { "$($r.L) $($r.T) $w $ht"; exit 0 }
$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
$ok = [W]::PrintWindow($h, $hdc, 2)
$g.ReleaseHdc($hdc); $g.Dispose()
if (-not $ok) { [Console]::Error.WriteLine("PrintWindow failed"); exit 1 }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
Write-Output "captured window -> $Out"
"#;

#[cfg(windows)]
fn powershell(mode: &str, out: &str) -> Result<String> {
    use std::path::PathBuf;
    std::fs::create_dir_all("target/xtask")?;
    let script = std::fs::canonicalize("target/xtask")
        .unwrap_or_else(|_| PathBuf::from("target/xtask"))
        .join("winshot.ps1");
    std::fs::write(&script, WIN_PS1)?;
    let o = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .args(["-Out", out, "-Title", &title(), "-Mode", mode])
        .output()
        .context("running powershell")?;
    if !o.status.success() {
        bail!("{}", String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn x11_window() -> Result<String> {
    if !have("xdotool", "version") {
        bail!("window capture on Linux needs xdotool (X11) plus ImageMagick `import` or `xwd`+`convert`; install them, or capture manually");
    }
    let o = Command::new("xdotool")
        .args(["search", "--name", &format!("^{}", title())])
        .output()?;
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .next()
        .map(String::from)
        .context("no Minecraft window found (Wayland-only sessions are not supported)")
}

pub fn screenshot(args: &[String]) -> Result<i32> {
    let out = args.first().map(String::as_str).unwrap_or("mc.png");
    capture(out)?;
    Ok(0)
}

#[cfg(target_os = "macos")]
fn capture(out: &str) -> Result<()> {
    let st = crate::util::status(Command::new(swift_tool("mc-window-shot")?).arg(out))?;
    if !st.success() {
        bail!("no Minecraft window");
    }
    Ok(())
}

#[cfg(windows)]
fn capture(out: &str) -> Result<()> {
    println!("{}", powershell("shot", out)?);
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn capture(out: &str) -> Result<()> {
    let id = x11_window()?;
    if have("import", "-version") {
        let st = crate::util::status(Command::new("import").args(["-window", &id, out]))?;
        if st.success() {
            println!("captured window {id} -> {out}");
            return Ok(());
        }
        bail!("import failed");
    }
    if have("xwd", "-version") && (have("convert", "-version") || have("magick", "-version")) {
        let conv = if have("magick", "-version") {
            "magick"
        } else {
            "convert"
        };
        let xwd = Command::new("xwd").args(["-silent", "-id", &id]).output()?;
        let mut c = Command::new(conv)
            .args(["xwd:-", out])
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        std::io::Write::write_all(c.stdin.as_mut().expect("piped stdin"), &xwd.stdout)?;
        drop(c.stdin.take());
        if c.wait()?.success() {
            println!("captured window {id} -> {out}");
            return Ok(());
        }
        bail!("convert failed");
    }
    bail!("need ImageMagick `import` (or `xwd` + `convert`) to capture a window");
}

pub fn bounds() -> Result<i32> {
    println!("{}", window_bounds()?);
    Ok(0)
}

#[cfg(target_os = "macos")]
fn window_bounds() -> Result<String> {
    let o = Command::new(swift_tool("mc-window-bounds")?).output()?;
    if !o.status.success() {
        bail!("no Minecraft window");
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

#[cfg(windows)]
fn window_bounds() -> Result<String> {
    powershell("bounds", "")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn window_bounds() -> Result<String> {
    let id = x11_window()?;
    let o = Command::new("xdotool")
        .args(["getwindowgeometry", "--shell", &id])
        .output()?;
    let s = String::from_utf8_lossy(&o.stdout);
    let get = |k: &str| {
        s.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .unwrap_or("0")
            .to_string()
    };
    Ok(format!(
        "{} {} {} {}",
        get("X"),
        get("Y"),
        get("WIDTH"),
        get("HEIGHT")
    ))
}
