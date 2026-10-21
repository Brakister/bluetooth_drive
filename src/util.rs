// Utilitarios gerais: strings UTF-16, tempo, log em arquivo com rotacao.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn ts(epoch: u64) -> String {
    // Converte epoch -> "YYYY-MM-DD HH:MM:SS" em UTC (sem dependencias externas).
    let days = epoch / 86_400;
    let secs = epoch % 86_400;
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

/// Epoch UTC -> "YYYY-MM-DD HH:MM:SS" na hora LOCAL do sistema.
pub fn ts_local(epoch: u64) -> String {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    const EPOCH_1601: u64 = 11_644_473_600;
    let ft = (epoch + EPOCH_1601) * 10_000_000;
    let src = FILETIME {
        dwLowDateTime: (ft & 0xFFFF_FFFF) as u32,
        dwHighDateTime: (ft >> 32) as u32,
    };
    let mut dst = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let ok = unsafe { FileTimeToLocalFileTime(&src, &mut dst) };
    if ok == 0 {
        return ts(epoch);
    }
    let local = (((dst.dwHighDateTime as u64) << 32) | dst.dwLowDateTime as u64) / 10_000_000;
    ts(local.saturating_sub(EPOCH_1601))
}

/// Algoritmo de Howard Hinnant: dias desde 1970 -> data civil.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn log_dir() -> Option<PathBuf> {
    for c in [
        std::env::var("PROGRAMDATA").ok().map(PathBuf::from),
        Some(PathBuf::from(r"C:\ProgramData")),
    ]
    .into_iter()
    .flatten()
    {
        if c.is_dir() {
            return Some(c.join("BTGuard"));
        }
    }
    None
}

pub fn log_file() -> Option<PathBuf> {
    log_dir().map(|d| d.join("btguard.log"))
}

/// Log simples em arquivo. Silencioso se nao houver permissao (ex.: sessao de usuario).
pub fn log(level: &str, msg: &str) {
    let line = format!("{} [{}] {}\n", ts_local(now_epoch()), level, msg);
    if let Some(f) = log_file() {
        if let Some(dir) = f.parent() {
            let _ = fs::create_dir_all(dir);
        }
        // Rotacao simples: se passar de 512 KB, renomeia para .1
        if let Ok(md) = fs::metadata(&f) {
            if md.len() > 512 * 1024 {
                let _ = fs::rename(&f, f.with_extension("log.1"));
            }
        }
        if let Ok(mut h) = OpenOptions::new().create(true).append(true).open(&f) {
            let _ = h.write_all(line.as_bytes());
        }
    }
    // Tambem no console, para_execucao em modo watch.
    eprint!("{line}");
}
