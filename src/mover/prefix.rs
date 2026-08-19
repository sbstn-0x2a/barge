//! Proton-Prefix nach dem Verschieben korrigieren (§4.3).
//!
//! `compatdata/<AppID>/pfx/dosdevices/` enthält Symlinks (`c:`, `z:`, ...),
//! von denen einige absolute Pfade in die *alte* Library enthalten können.
//! Diese werden auf den neuen Library-Pfad umgeschrieben. Die Registry-Dateien
//! (`system.reg`, `user.reg`) werden bewusst **nicht** angefasst — Proton bezieht
//! den Prefix-Pfad zur Laufzeit aus `STEAM_COMPAT_DATA_PATH`.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

/// Schreibt dosdevices-Symlinks unter `compat_root` um, deren Ziel mit
/// `old_lib` beginnt, sodass sie auf `new_lib` zeigen. Entfernt außerdem eine
/// evtl. vorhandene `pfx.lock` (Rest einer laufenden Session). Gibt die Anzahl
/// korrigierter Symlinks zurück.
pub fn fix_prefix(compat_root: &Path, old_lib: &Path, new_lib: &Path) -> std::io::Result<u32> {
    let dosdevices = compat_root.join("pfx/dosdevices");
    let mut fixed = 0;

    if dosdevices.is_dir() {
        let old = old_lib.to_string_lossy().to_string();
        let new = new_lib.to_string_lossy().to_string();
        for e in fs::read_dir(&dosdevices)? {
            let p = e?.path();
            if !fs::symlink_metadata(&p)?.file_type().is_symlink() {
                continue;
            }
            let target = fs::read_link(&p)?;
            let ts = target.to_string_lossy().to_string();
            if ts.starts_with(&old) {
                let rewritten = ts.replacen(&old, &new, 1);
                fs::remove_file(&p)?;
                symlink(&rewritten, &p)?;
                fixed += 1;
            }
        }
    }

    let lock = compat_root.join("pfx.lock");
    if lock.exists() {
        fs::remove_file(lock)?;
    }
    Ok(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn schreibt_dosdevices_um_und_entfernt_lock() {
        let dir = std::env::temp_dir().join(format!("barge_prefix_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let compat = dir.join("compatdata/1234567");
        let dd = compat.join("pfx/dosdevices");
        fs::create_dir_all(&dd).unwrap();

        let old_lib = PathBuf::from("/old/SteamLibrary");
        let new_lib = PathBuf::from("/mnt/new/SteamLibrary");

        // c: -> relativer Prefix-Pfad (bleibt), d: -> absoluter alter Pfad (fix),
        // z: -> /  (bleibt).
        symlink("../drive_c", dd.join("c:")).unwrap();
        symlink(
            format!("{}/steamapps/common/Game", old_lib.display()),
            dd.join("d:"),
        )
        .unwrap();
        symlink("/", dd.join("z:")).unwrap();
        fs::write(compat.join("pfx.lock"), b"").unwrap();

        let fixed = fix_prefix(&compat, &old_lib, &new_lib).unwrap();
        assert_eq!(fixed, 1);

        assert_eq!(
            fs::read_link(dd.join("d:")).unwrap(),
            PathBuf::from(format!("{}/steamapps/common/Game", new_lib.display()))
        );
        assert_eq!(fs::read_link(dd.join("c:")).unwrap(), PathBuf::from("../drive_c"));
        assert_eq!(fs::read_link(dd.join("z:")).unwrap(), PathBuf::from("/"));
        assert!(!compat.join("pfx.lock").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
