//! Verifikation nach dem Kopieren (§7.3).
//!
//! „Schnell" (Default): reguläre Dateien nach relativem Pfad, Länge und
//! mtime-Sekunden vergleichen. Symlinks werden übersprungen — der Prefix-Fix
//! (§4.3) verändert dosdevices-Ziele im `.partial`, sie dürfen also abweichen.
//! Bei Abweichung wird der Move abgebrochen; das `.partial` bleibt erhalten und
//! die Quelle unangetastet (§7.3).

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Vergleicht zwei Bäume schnell. `Ok(())` bei Übereinstimmung, sonst die erste
/// Abweichung als Text.
pub fn verify_quick(src: &Path, dst: &Path) -> Result<(), String> {
    use crate::i18n::trf;
    let s = collect(src)?;
    let d = collect(dst)?;
    if s.len() != d.len() {
        return Err(trf(
            "Dateianzahl weicht ab (Quelle {}, Ziel {})",
            "file count differs (source {}, target {})",
            &[&s.len().to_string(), &d.len().to_string()],
        ));
    }
    for (rel, (len, mtime)) in &s {
        match d.get(rel) {
            None => return Err(trf("fehlt am Ziel: {}", "missing at target: {}", &[rel])),
            Some((dlen, dmtime)) => {
                if dlen != len {
                    return Err(trf(
                        "Größe weicht ab: {} ({} vs {} Bytes)",
                        "size differs: {} ({} vs {} bytes)",
                        &[rel, &len.to_string(), &dlen.to_string()],
                    ));
                }
                if dmtime != mtime {
                    return Err(trf("mtime weicht ab: {}", "mtime differs: {}", &[rel]));
                }
            }
        }
    }
    Ok(())
}

/// Sammelt reguläre Dateien (relativer Pfad -> (Länge, mtime-Sekunden)).
fn collect(root: &Path) -> Result<HashMap<String, (u64, i64)>, String> {
    let mut map = HashMap::new();
    walk(root, root, &mut map)?;
    Ok(map)
}

fn walk(root: &Path, dir: &Path, map: &mut HashMap<String, (u64, i64)>) -> Result<(), String> {
    let rd = fs::read_dir(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    for entry in rd {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let md = fs::symlink_metadata(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let ft = md.file_type();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            walk(root, &path, map)?;
        } else if ft.is_file() {
            let rel = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());
            map.insert(rel, (md.len(), md.mtime()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("barge_verify_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn erkennt_gleich_und_abweichung() {
        let dir = tmp("q");
        let a = dir.join("a");
        let b = dir.join("b");
        fs::create_dir_all(a.join("sub")).unwrap();
        fs::create_dir_all(b.join("sub")).unwrap();
        fs::write(a.join("x"), b"hallo").unwrap();
        fs::write(b.join("x"), b"hallo").unwrap();
        fs::write(a.join("sub/y"), b"welt").unwrap();
        fs::write(b.join("sub/y"), b"welt").unwrap();

        // mtimes angleichen (kopierte Dateien hätten identische mtime).
        let t = fs::metadata(a.join("x")).unwrap().modified().unwrap();
        filetime_set(&b.join("x"), t);
        let t2 = fs::metadata(a.join("sub/y")).unwrap().modified().unwrap();
        filetime_set(&b.join("sub/y"), t2);

        assert!(verify_quick(&a, &b).is_ok());

        // Abweichende Größe.
        let mut f = fs::OpenOptions::new().append(true).open(b.join("x")).unwrap();
        f.write_all(b"!").unwrap();
        assert!(verify_quick(&a, &b).is_err());

        fs::remove_dir_all(&dir).ok();
    }

    // Setzt die mtime einer Datei auf die einer anderen (nur für den Test).
    fn filetime_set(path: &Path, t: std::time::SystemTime) {
        use std::os::unix::fs::MetadataExt;
        let secs = t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let src_md = fs::metadata(path).unwrap();
        let atime = src_md.atime();
        set_times(path, atime, secs);
    }

    fn set_times(path: &Path, atime: i64, mtime: i64) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        #[repr(C)]
        struct Timespec {
            tv_sec: i64,
            tv_nsec: i64,
        }
        extern "C" {
            fn utimensat(dirfd: i32, path: *const i8, times: *const Timespec, flags: i32) -> i32;
        }
        let c = CString::new(path.as_os_str().as_bytes()).unwrap();
        let times = [
            Timespec { tv_sec: atime, tv_nsec: 0 },
            Timespec { tv_sec: mtime, tv_nsec: 0 },
        ];
        const AT_FDCWD: i32 = -100;
        unsafe {
            utimensat(AT_FDCWD, c.as_ptr(), times.as_ptr(), 0);
        }
    }
}
