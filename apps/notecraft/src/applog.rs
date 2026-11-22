//! The app's log file. A release build on Windows has no console, so without this a crash, a
//! GPU error or a failed start leaves nothing behind.
//!
//! `notecraft.log` sits next to the active settings file: normally `local/` in a development
//! checkout and `%APPDATA%\NoteCraft` for an installed Windows build. Each line is
//! `UTC time, level, text`. Structured telemetry sessions are stored separately under the sibling
//! `telemetry/` directory. The file is opened for every line and never held open, so the last
//! line before a hard crash is on disk. When it passes 1 MB it is renamed to `notecraft.log.old`.
//! Warnings and errors from every library are kept, and so is the information from NoteCraft,
//! eframe and the window and GPU glue, which is where start-up problems show.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use web_time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 1_000_000;

static PATH: OnceLock<PathBuf> = OnceLock::new();

/// Sets the log file up, rotates it if it is large, and routes the `log` crate and panics to it.
pub fn init(dir: PathBuf, debug: bool) {
    let file = dir.join("notecraft.log");
    let _ = std::fs::create_dir_all(&dir);
    rotate_if_large(&file);
    let _ = PATH.set(file);
    if log::set_logger(&Logger).is_ok() {
        log::set_max_level(if debug || rust_log_requests_debug() { log::LevelFilter::Debug } else { log::LevelFilter::Info });
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        line("PANIC", &format!("{info}\n{}", std::backtrace::Backtrace::force_capture()));
        previous(info);
    }));
}

fn rust_log_requests_debug() -> bool {
    std::env::var("RUST_LOG").ok().is_some_and(|value| rust_log_requests_debug_from(&value))
}

fn rust_log_requests_debug_from(value: &str) -> bool {
    value
        .split(',')
        .map(str::trim)
        .any(|directive| matches!(directive, "debug" | "trace") || directive.ends_with("=debug") || directive.ends_with("=trace"))
}

/// Moves the log aside when it is over the size limit, so the next line starts a new file.
fn rotate_if_large(file: &Path) {
    if std::fs::metadata(file).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(file, file.with_file_name("notecraft.log.old"));
    }
}

/// Appends one `time level text` line to `file`, opening it for this line only.
fn append_to(file: &Path, time: &str, level: &str, text: &str) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file)?;
    writeln!(f, "{time} {level} {text}")
}

/// Appends one line, `time level text`. Never fails the caller.
pub fn line(level: &str, text: &str) {
    let Some(path) = PATH.get() else { return };
    let _ = append_to(path, &utc_now(), level, text);
}

pub fn info(text: &str) {
    line("INFO", text);
}

struct Logger;

/// Which records are kept: every warning and error, and the information from NoteCraft and from
/// the window and GPU glue. Other libraries' information is left out.
fn keeps(target: &str, level: log::Level) -> bool {
    level <= log::Level::Warn
        || target.starts_with("notecraft")
        || target.starts_with("eframe")
        || target.starts_with("egui_wgpu")
        || target.starts_with("egui_winit")
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        keeps(m.target(), m.level())
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            line(r.level().as_str(), &format!("{}: {}", r.target(), r.args()));
        }
    }

    fn flush(&self) {}
}

/// `YYYY-MM-DD HH:MM:SS` in UTC, from the system clock.
fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format_utc(secs)
}

fn format_utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(format_utc(1_792_340_000), "2026-10-18 16:13:20");
    }

    #[test]
    fn a_large_log_is_moved_aside_and_a_small_one_is_kept() {
        let t = notecraft_store::testing::TempDir::new("applog-rotate");
        let file = t.path().join("notecraft.log");
        std::fs::write(&file, vec![b'x'; MAX_BYTES as usize]).expect("a log at the limit");
        rotate_if_large(&file);
        assert!(file.exists(), "a log at exactly the limit is kept");

        std::fs::write(&file, vec![b'x'; MAX_BYTES as usize + 1]).expect("a log over the limit");
        rotate_if_large(&file);
        assert!(!file.exists(), "a log over the limit is moved aside");
        assert_eq!(std::fs::metadata(t.path().join("notecraft.log.old")).expect("the old log").len(), MAX_BYTES + 1);
    }

    #[test]
    fn each_line_is_time_level_and_text_and_lines_append() {
        let t = notecraft_store::testing::TempDir::new("applog-append");
        let file = t.path().join("notecraft.log");
        append_to(&file, "2026-11-21 10:00:00", "INFO", "started").expect("first line");
        append_to(&file, "2026-11-21 10:00:01", "WARN", "slow start").expect("second line");
        assert_eq!(std::fs::read_to_string(&file).expect("the log"), "2026-11-21 10:00:00 INFO started\n2026-11-21 10:00:01 WARN slow start\n");
    }

    #[test]
    fn warnings_and_our_information_are_kept_and_other_libraries_information_is_not() {
        assert!(keeps("hyper::client", log::Level::Error), "errors from any library are kept");
        assert!(keeps("hyper::client", log::Level::Warn), "warnings from any library are kept");
        assert!(!keeps("hyper::client", log::Level::Info), "a library's information is left out");
        for ours in ["notecraft", "notecraft_ui_egui::app", "eframe::native", "egui_wgpu::renderer", "egui_winit::clipboard"] {
            assert!(keeps(ours, log::Level::Info), "{ours} information is kept");
        }
        assert!(!keeps("egui::context", log::Level::Info), "egui's own information is left out, eframe's is kept");
    }

    #[test]
    fn recognizes_debug_rust_log_directives() {
        for value in ["debug", "trace", "notecraft_ui_egui=debug", "notecraft_ui_egui=trace"] {
            assert!(rust_log_requests_debug_from(value));
        }
        assert!(!rust_log_requests_debug_from("info,notecraft_ui_egui=warn"));
    }
}
