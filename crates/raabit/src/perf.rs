//! Measurement hook for the speed budgets (`docs/perf.md`).
//!
//! Enabled by `--perf-report <file>` (or `RAABIT_PERF=<file>`); without it `raabit` holds a
//! `None` and pays one branch per frame. The hook records what only the process can know:
//! the time of the first interactive frame and the duration of every `update()` call, each
//! stamped with wall-clock time so `cargo xtask perf` can attribute frames to the phase it
//! was driving. Idle CPU and RSS are read from `/proc` by the driver. The report is written
//! when the window closes. The driver asks for that by creating `<report>.stop` and moving
//! the mouse: the next frame sees the file and closes the window.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const ENV: &str = "RAABIT_PERF";

pub struct Perf {
    path: std::path::PathBuf,
    start: Instant,
    start_unix_ms: u128,
    first_frame_ms: Option<f64>,
    /// `(unix ms at the end of the frame, update() duration in µs)`.
    frames: Vec<(u128, u64)>,
    frame_start: Option<Instant>,
}

impl Perf {
    /// `start` is taken at the top of `main`, as early as the process can see.
    pub fn new(path: std::path::PathBuf, start: Instant) -> Self {
        let start_unix_ms = unix_ms().saturating_sub(start.elapsed().as_millis());
        Self {
            path,
            start,
            start_unix_ms,
            first_frame_ms: None,
            frames: Vec::with_capacity(4096),
            frame_start: None,
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame_start = Some(Instant::now());
    }

    /// `interactive`: this frame rendered the plugin view, so the user can act on it.
    pub fn end_frame(&mut self, interactive: bool) {
        let Some(began) = self.frame_start.take() else {
            return;
        };
        self.frames
            .push((unix_ms(), began.elapsed().as_micros() as u64));
        if interactive && self.first_frame_ms.is_none() {
            self.first_frame_ms = Some(self.start.elapsed().as_secs_f64() * 1000.0);
        }
    }

    pub fn report(&self) -> String {
        let frames = self
            .frames
            .iter()
            .map(|(t, us)| format!("[{t},{us}]"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"start_unix_ms\":{},\"first_frame_ms\":{},\"frames\":[{frames}]}}\n",
            self.start_unix_ms,
            self.first_frame_ms
                .map_or("null".to_owned(), |ms| format!("{ms:.1}")),
        )
    }

    /// The driver's request to finish: `<report>.stop` exists.
    pub fn stop_requested(&self) -> bool {
        let mut stop = self.path.clone().into_os_string();
        stop.push(".stop");
        std::path::Path::new(&stop).exists()
    }

    pub fn write(&self) {
        if let Err(err) = std::fs::write(&self.path, self.report()) {
            eprintln!("raabit: cannot write {}: {err}", self.path.display());
        }
    }
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_has_first_frame_and_frames() {
        let mut perf = Perf::new("unused".into(), Instant::now());
        perf.begin_frame();
        perf.end_frame(false);
        assert!(perf.report().contains("\"first_frame_ms\":null"));
        perf.begin_frame();
        perf.end_frame(true);
        let report = perf.report();
        assert!(!report.contains("null"));
        assert_eq!(report.matches('[').count(), 3);
    }
}
