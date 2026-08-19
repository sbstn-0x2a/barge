//! Kleine Helfer ohne externe Crates: Größenformatierung, reale
//! Verzeichnisgröße (§5.4) und freier Platz per `statvfs`.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Formatiert Bytes in binären Einheiten (GiB/TiB) mit einer Nachkommastelle
/// und deutschem Dezimalkomma — passend zu den UI-Mockups im Handout (§8.1),
/// z. B. `148,2 GiB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", bytes)
    } else {
        // Deutsches Dezimalkomma statt Punkt.
        format!("{:.1} {}", value, UNITS[unit]).replace('.', ",")
    }
}

/// Reale Belegung auf dem Datenträger (`st_blocks × 512`), rekursiv über einen
/// Baum. Bewusst *nicht* die apparent size und *nicht* `SizeOnDisk` aus dem ACF
/// (§5.4: dort wurden 48,3 GB für ein 98-MB-Spiel gemeldet).
///
/// Symlinks werden nicht dereferenziert (sie belegen selbst kaum Platz).
/// Fehler beim Betreten einzelner Einträge werden übersprungen, damit ein
/// unlesbares Unterverzeichnis nicht die gesamte Summe scheitern lässt.
pub fn dir_real_size(path: &Path) -> u64 {
    let md = match std::fs::symlink_metadata(path) {
        Ok(md) => md,
        Err(_) => return 0,
    };
    let ft = md.file_type();
    if ft.is_symlink() {
        return 0;
    }
    if ft.is_file() {
        return md.blocks() * 512;
    }
    if ft.is_dir() {
        let mut total = 0u64;
        if let Ok(rd) = std::fs::read_dir(path) {
            for entry in rd.flatten() {
                total += dir_real_size(&entry.path());
            }
        }
        return total;
    }
    0
}

#[repr(C)]
struct StatVfs {
    f_bsize: u64,
    f_frsize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_favail: u64,
    f_fsid: u64,
    f_flag: u64,
    f_namemax: u64,
    __spare: [i32; 6],
}

extern "C" {
    fn statvfs(path: *const std::os::raw::c_char, buf: *mut StatVfs) -> i32;
}

/// Gesamt- und (für den aufrufenden Nutzer) verfügbarer Platz eines
/// Dateisystems in Bytes. Nutzt `f_bavail` (dem Nicht-root verfügbar), nicht
/// `f_bfree`. Gibt `None` zurück, wenn `statvfs` scheitert.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let cpath = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: gültiger C-String, `buf` ist ausreichend dimensioniert.
    let mut buf: StatVfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { statvfs(cpath.as_ptr(), &mut buf) };
    if rc != 0 {
        return None;
    }
    let total = buf.f_blocks.saturating_mul(buf.f_frsize);
    let avail = buf.f_bavail.saturating_mul(buf.f_frsize);
    Some((total, avail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatiert_groessen() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1,0 KiB");
        assert_eq!(human_size(1536), "1,5 KiB");
        assert_eq!(human_size(159_161_991_168), "148,2 GiB");
    }

    #[test]
    fn disk_space_auf_root() {
        // "/" existiert immer; wir prüfen nur, dass ein sinnvoller Wert kommt.
        let (total, avail) = disk_space(Path::new("/")).expect("statvfs auf /");
        assert!(total > 0);
        assert!(avail <= total);
    }
}
