//! Kopier-Engine (§6) — der kritische Teil.
//!
//! Harte Regeln (§6.1), nicht verhandelbar:
//! - **ein** Worker-Thread, sequenziell, Queue Depth 1 (dieser Code läuft im
//!   Aufrufer-Thread; es gibt keine Parallelität über Dateien)
//! - Bandbreite per Drossel begrenzt ([`Throttle`])
//! - periodischer `fsync` alle ~64 MB geschriebener Daten
//!
//! Primitive (§6.2/§6.3): `copy_file_range` in 4-MiB-Chunks mit Fallback auf
//! `read_at`/`write_at`, `posix_fadvise(DONTNEED)` auf die Quelle, `fsync` je
//! Zieldatei. Baum-Sonderfälle (§6.5): Symlinks werden reproduziert, Hardlinks
//! über eine Inode-Map, Permissions und mtime übernommen, und — anders als im
//! Prototyp — **Sparse-Löcher via `SEEK_HOLE`/`SEEK_DATA` erhalten**.

use std::collections::HashMap;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::os::unix::fs::{symlink, FileExt, MetadataExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use super::throttle::Throttle;

const CHUNK: usize = 4 * 1024 * 1024; // §6.2
const FSYNC_EVERY: u64 = 64 * 1024 * 1024; // §6.1

// Linux-Konstanten (kein libc-Crate).
const SEEK_DATA: i32 = 3;
const SEEK_HOLE: i32 = 4;
const POSIX_FADV_DONTNEED: i32 = 4;
const ENXIO: i32 = 6;
const EINVAL: i32 = 22;
const EINTR: i32 = 4;
const EXDEV: i32 = 18;
const ENOSYS: i32 = 38;

extern "C" {
    fn copy_file_range(
        fd_in: i32,
        off_in: *mut i64,
        fd_out: i32,
        off_out: *mut i64,
        len: usize,
        flags: u32,
    ) -> isize;
    fn posix_fadvise(fd: i32, offset: i64, len: i64, advice: i32) -> i32;
    fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
    fn futimens(fd: i32, times: *const Timespec) -> i32;
}

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub files: u64,
    pub dirs: u64,
    pub symlinks: u64,
    pub hardlinks: u64,
    pub bytes: u64,
    /// Anzahl übersprungener Sparse-Löcher (§6.5).
    pub holes: u64,
    pub cfr_ok: u64,
    pub cfr_fallback: u64,
}

/// Laufzeitzustand eines Kopier-Jobs. Ein einziger Worker (dieser Aufruf),
/// keine Parallelität (§6.1). Der Fortschritts-Callback `F` erhält die
/// laufenden Stats und die gemessene MB/s.
pub struct Copier<F: FnMut(&Stats, f64)> {
    throttle: Throttle,
    stats: Stats,
    inodes: HashMap<(u64, u64), PathBuf>,
    progress: F,
}

impl<F: FnMut(&Stats, f64)> Copier<F> {
    /// `rate_bytes_per_sec == 0` bedeutet unbegrenzt.
    pub fn new(rate_bytes_per_sec: u64, progress: F) -> Self {
        Copier {
            throttle: Throttle::new(rate_bytes_per_sec),
            stats: Stats::default(),
            inodes: HashMap::new(),
            progress,
        }
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    pub fn measured_mbps(&self) -> f64 {
        self.throttle.measured_mbps()
    }

    /// Kopiert einen Baum (Datei, Verzeichnis oder Symlink) rekursiv nach `dst`.
    /// Erhält Symlinks, Hardlinks, Permissions, mtime und Sparse-Löcher.
    pub fn copy_tree(&mut self, src: &Path, dst: &Path) -> io::Result<()> {
        let md = fs::symlink_metadata(src)?;
        let ft = md.file_type();

        if ft.is_symlink() {
            let target = fs::read_link(src)?;
            let _ = fs::remove_file(dst);
            symlink(&target, dst)?;
            self.stats.symlinks += 1;
            return Ok(());
        }

        if ft.is_dir() {
            fs::create_dir_all(dst)?;
            self.stats.dirs += 1;
            let mut entries: Vec<_> = fs::read_dir(src)?.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                self.copy_tree(&e.path(), &dst.join(e.file_name()))?;
            }
            // Permissions und mtime des Verzeichnisses nach dem Befüllen setzen.
            fs::set_permissions(dst, md.permissions())?;
            if let Ok(f) = File::open(dst) {
                set_times(f.as_raw_fd(), &md);
            }
            return Ok(());
        }

        if !ft.is_file() {
            // Sockets/FIFOs/Devices kommen in Spielordnern nicht vor; ignorieren.
            return Ok(());
        }

        // Hardlink bereits gesehen? -> verlinken statt kopieren (§6.5).
        if md.nlink() > 1 {
            let key = (md.dev(), md.ino());
            if let Some(first) = self.inodes.get(&key) {
                let _ = fs::remove_file(dst);
                fs::hard_link(first, dst)?;
                self.stats.hardlinks += 1;
                return Ok(());
            }
            self.inodes.insert(key, dst.to_path_buf());
        }

        self.copy_file(src, dst, &md)
    }

    fn copy_file(&mut self, src: &Path, dst: &Path, md: &Metadata) -> io::Result<()> {
        let src_file = File::open(src)?;
        let dst_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dst)?;
        let src_fd = src_file.as_raw_fd();
        let dst_fd = dst_file.as_raw_fd();
        let size = md.len() as i64;

        let mut since_sync = 0u64;
        let mut use_cfr = true;

        // Datenbereiche kopieren, Löcher (Lücken dazwischen) zählen (§6.5).
        let segments = data_segments(src_fd, size)?;
        let mut prev_end = 0i64;
        for (start, end) in segments {
            if start > prev_end {
                self.stats.holes += 1;
            }
            self.copy_range(
                &src_file, &dst_file, start, end, &mut since_sync, &mut use_cfr,
            )?;
            prev_end = end;
        }
        if size > prev_end {
            self.stats.holes += 1; // abschließendes Loch
        }

        // Exakte Endgröße setzen — erhält ein evtl. abschließendes Loch (§6.5).
        dst_file.set_len(md.len())?;

        dst_file.sync_data()?; // §6.3 fsync je Zieldatei
        dst_file.set_permissions(md.permissions())?;
        set_times(dst_fd, md);
        // §6.3 Page Cache der Quelle wieder freigeben.
        unsafe { posix_fadvise(src_fd, 0, 0, POSIX_FADV_DONTNEED) };

        self.stats.files += 1;
        (self.progress)(&self.stats, self.throttle.measured_mbps());
        Ok(())
    }

    /// Kopiert einen Datenbereich `[start, end)`. Löcher zwischen den Bereichen
    /// werden nie beschrieben und bleiben am Ziel sparse.
    fn copy_range(
        &mut self,
        src_file: &File,
        dst_file: &File,
        start: i64,
        end: i64,
        since_sync: &mut u64,
        use_cfr: &mut bool,
    ) -> io::Result<()> {
        let src_fd = src_file.as_raw_fd();
        let dst_fd = dst_file.as_raw_fd();
        let mut off_in = start;
        let mut off_out = start;
        let mut remaining = (end - start) as u64;
        let mut buf = Vec::new();

        while remaining > 0 {
            let want = std::cmp::min(remaining, CHUNK as u64) as usize;

            let done: u64 = if *use_cfr {
                let r = unsafe {
                    copy_file_range(src_fd, &mut off_in, dst_fd, &mut off_out, want, 0)
                };
                if r < 0 {
                    let err = io::Error::last_os_error();
                    match err.raw_os_error() {
                        Some(EINTR) => continue,
                        // Ältere Kernel / exotische FS -> read/write-Fallback (§6.2).
                        Some(EXDEV) | Some(ENOSYS) => {
                            *use_cfr = false;
                            self.stats.cfr_fallback += 1;
                            continue;
                        }
                        _ => return Err(err),
                    }
                }
                if r == 0 {
                    break;
                }
                self.stats.cfr_ok += 1;
                r as u64
            } else {
                if buf.len() < want {
                    buf.resize(want, 0);
                }
                let n = src_file.read_at(&mut buf[..want], off_in as u64)?;
                if n == 0 {
                    break;
                }
                dst_file.write_at(&buf[..n], off_out as u64)?;
                off_in += n as i64;
                off_out += n as i64;
                n as u64
            };

            remaining -= done;
            *since_sync += done;
            self.stats.bytes += done;
            self.throttle.account(done);

            if *since_sync >= FSYNC_EVERY {
                dst_file.sync_data()?; // §6.1 Dirty Pages begrenzen
                *since_sync = 0;
                (self.progress)(&self.stats, self.throttle.measured_mbps());
            }
        }
        Ok(())
    }
}

/// Ermittelt die Datenbereiche einer (evtl. sparse) Datei über
/// `SEEK_DATA`/`SEEK_HOLE`. Fällt der Kernel/das FS darauf zurück, dass es die
/// nicht unterstützt (`EINVAL`), wird die Datei als ein einziger Datenbereich
/// behandelt.
fn data_segments(fd: i32, size: i64) -> io::Result<Vec<(i64, i64)>> {
    let mut segs = Vec::new();
    if size == 0 {
        return Ok(segs);
    }
    let mut off = 0i64;
    loop {
        let data = unsafe { lseek(fd, off, SEEK_DATA) };
        if data < 0 {
            let e = io::Error::last_os_error();
            match e.raw_os_error() {
                Some(ENXIO) => break, // kein Datenbereich mehr
                Some(EINVAL) => {
                    // SEEK_DATA nicht unterstützt: ganze Datei als ein Bereich.
                    return Ok(vec![(0, size)]);
                }
                _ => return Err(e),
            }
        }
        if data >= size {
            break;
        }
        let hole = unsafe { lseek(fd, data, SEEK_HOLE) };
        let end = if hole < 0 { size } else { hole.min(size) };
        segs.push((data, end));
        off = end;
        if off >= size {
            break;
        }
    }
    Ok(segs)
}

/// atime und mtime der Quelle auf die Zieldatei/-verzeichnis übernehmen (§6.5).
fn set_times(fd: i32, md: &Metadata) {
    let times = [
        Timespec {
            tv_sec: md.atime(),
            tv_nsec: md.atime_nsec(),
        },
        Timespec {
            tv_sec: md.mtime(),
            tv_nsec: md.mtime_nsec(),
        },
    ];
    // Fehler bewusst ignoriert: mtime-Erhalt ist wünschenswert, aber kein
    // Grund, einen ansonsten erfolgreichen Kopiervorgang scheitern zu lassen.
    unsafe { futimens(fd, times.as_ptr()) };
}

/// `fsync` auf ein Verzeichnis, damit neue Einträge dauerhaft werden (§6.3).
pub fn fsync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

/// Liegen zwei Pfade auf demselben Blockgerät? Dann ist `rename(2)` instantan
/// und statt Kopieren zu verwenden (§6.4). Für den Ziel-Pfad wird der Parent
/// herangezogen, da die Zieldatei selbst noch nicht existiert.
pub fn same_device(a: &Path, b: &Path) -> io::Result<bool> {
    let dev_a = fs::metadata(a)?.dev();
    let b_probe = if b.exists() { b } else { b.parent().unwrap_or(b) };
    let dev_b = fs::metadata(b_probe)?.dev();
    Ok(dev_a == dev_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    use std::os::unix::fs::PermissionsExt;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("barge_test_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn noop_progress(_: &Stats, _: f64) {}

    #[test]
    fn kopiert_datei_inhalt_und_rechte() {
        let dir = tmpdir("basic");
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        fs::write(&src, b"hallo welt").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o640)).unwrap();

        let mut c = Copier::new(0, noop_progress);
        c.copy_tree(&src, &dst).unwrap();

        assert_eq!(fs::read(&dst).unwrap(), b"hallo welt");
        assert_eq!(fs::metadata(&dst).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(c.stats().files, 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn erhaelt_sparse_loecher() {
        let dir = tmpdir("sparse");
        let src = dir.join("sparse.bin");
        let dst = dir.join("sparse_copy.bin");

        // 4 KiB Daten, dann ein 8-MiB-Loch, dann wieder 4 KiB Daten.
        let mut f = File::create(&src).unwrap();
        f.write_all(&[0xAB; 4096]).unwrap();
        f.seek(SeekFrom::Start(8 * 1024 * 1024)).unwrap();
        f.write_all(&[0xCD; 4096]).unwrap();
        f.sync_all().unwrap();
        drop(f);

        let src_md = fs::metadata(&src).unwrap();
        let src_apparent = src_md.len();
        let src_blocks = src_md.blocks();

        let mut c = Copier::new(0, noop_progress);
        c.copy_tree(&src, &dst).unwrap();

        let dst_md = fs::metadata(&dst).unwrap();
        // Gleiche logische Größe.
        assert_eq!(dst_md.len(), src_apparent);
        // Inhalt identisch.
        assert_eq!(fs::read(&src).unwrap(), fs::read(&dst).unwrap());
        // Das Loch bleibt erhalten: real belegte Blöcke bleiben weit unter der
        // apparent size (das genau war der Prototyp-Bug, §6.5).
        assert!(
            dst_md.blocks() <= src_blocks + 16,
            "Ziel nicht sparse: {} Blöcke (Quelle {}), apparent {} B",
            dst_md.blocks(),
            src_blocks,
            src_apparent
        );
        // Das mittlere Loch wurde erkannt und nur die Daten kopiert.
        assert!(c.stats().holes >= 1, "kein Loch erkannt");
        assert!(c.stats().bytes < src_apparent, "hat das Loch mitkopiert");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reproduziert_symlink_und_hardlink() {
        let dir = tmpdir("links");
        let src = dir.join("game");
        let dst = dir.join("game_copy");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("bin"), b"data").unwrap();
        // Hardlink auf dieselbe Datei.
        fs::hard_link(src.join("bin"), src.join("bin.hl")).unwrap();
        // Symlink innerhalb des Ordners.
        symlink("bin", src.join("link")).unwrap();

        let mut c = Copier::new(0, noop_progress);
        c.copy_tree(&src, &dst).unwrap();

        // Symlink bleibt Symlink und wird nicht dereferenziert.
        let lmd = fs::symlink_metadata(dst.join("link")).unwrap();
        assert!(lmd.file_type().is_symlink());
        assert_eq!(fs::read_link(dst.join("link")).unwrap(), PathBuf::from("bin"));

        // Hardlink: identische Inode am Ziel.
        let a = fs::metadata(dst.join("bin")).unwrap().ino();
        let b = fs::metadata(dst.join("bin.hl")).unwrap().ino();
        assert_eq!(a, b, "Hardlink nicht reproduziert");
        assert_eq!(c.stats().hardlinks, 1);
        assert_eq!(c.stats().symlinks, 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn same_device_erkennt_gleiches_fs() {
        let dir = tmpdir("dev");
        let a = dir.join("a");
        fs::write(&a, b"x").unwrap();
        // b existiert noch nicht -> Parent wird geprüft.
        let b = dir.join("b");
        assert!(same_device(&a, &b).unwrap());
        fs::remove_dir_all(&dir).ok();
    }
}
