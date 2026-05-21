//! Host implementation of the `highbeam:platform` module — metadata only,
//! no capability required.

use std::sync::OnceLock;

use rquickjs::{Ctx, Function, Result as JsResult, module::ModuleDef};

pub struct PlatformModule;

impl ModuleDef for PlatformModule {
    fn declare(decl: &rquickjs::module::Declarations<'_>) -> JsResult<()> {
        decl.declare("os")?;
        decl.declare("arch")?;
        decl.declare("version")?;
        decl.declare("isMacOS")?;
        decl.declare("isLinux")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> JsResult<()> {
        exports.export("os", normalized_os())?;
        exports.export("arch", std::env::consts::ARCH)?;
        exports.export("version", os_version())?;
        let is_macos = Function::new(ctx.clone(), || cfg!(target_os = "macos"))?;
        let is_linux = Function::new(ctx.clone(), || cfg!(target_os = "linux"))?;
        exports.export("isMacOS", is_macos)?;
        exports.export("isLinux", is_linux)?;
        Ok(())
    }
}

/// `std::env::consts::OS` already returns `"macos"`/`"linux"`; wrapper
/// centralises the contract for future normalisation.
fn normalized_os() -> &'static str {
    std::env::consts::OS
}

/// OS version, cached after first lookup. Returns `"unknown"` on any
/// failure — the plugin contract is "best-effort, never throws".
fn os_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(detect_os_version).as_str()
}

fn detect_os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            && out.status.success()
        {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if !s.is_empty() {
                return s;
            }
        }
        "unknown".into()
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(out) = std::process::Command::new("/usr/bin/uname")
            .arg("-r")
            .output()
            && out.status.success()
        {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if !s.is_empty() {
                return s;
            }
        }
        "unknown".into()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        "unknown".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_version_never_empty() {
        let v = os_version();
        assert!(!v.is_empty());
    }
}
