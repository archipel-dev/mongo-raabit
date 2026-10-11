//! Measurement hook for the speed budgets (`docs/perf.md`).
//!
//! Enabled by `--perf-report <file>` (or `RAABIT_PERF=<file>`); without it `raabit` holds a
//! `None` and pays one branch per frame. The hook records what only the process can know:
//! the time of the first interactive frame and the duration of every `update()` call, each
//! stamped with wall-clock time so `cargo xtask perf` can attribute frames to the phase it
//! was driving. Idle CPU and RSS are read from `/proc` by the driver. The report is written
//! when the window closes. The driver asks for that by creating `<report>.stop` and moving
//! the mouse: the next frame sees the file and closes the window.

use archipel_sdk::api::{
    CommandResult, Contributions, Plugin, PluginError, PluginHost, PluginManifest, Value,
    ViewContext, ViewFrame,
};
use archipel_sdk::app::CMD_QUIT;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const ENV: &str = "RAABIT_PERF";

pub struct Perf {
    path: std::path::PathBuf,
    start: Instant,
    start_unix_ms: u128,
    first_frame_ms: Option<f64>,
    /// `(unix ms at the start of the frame, plugin time of the frame in µs)`.
    frames: Vec<(u128, u64)>,
    /// Plugin time accumulated in the frame in progress.
    current: Option<(u128, Duration)>,
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
            current: None,
        }
    }

    /// `poll` runs once per host frame: it closes the previous frame and opens the next.
    pub fn begin_frame(&mut self) {
        self.flush();
        self.current = Some((unix_ms(), Duration::ZERO));
    }

    /// Adds plugin time to the frame in progress. `interactive`: a view was rendered, so the
    /// user can act on the window.
    pub fn add(&mut self, spent: Duration, interactive: bool) {
        if let Some((_, total)) = &mut self.current {
            *total += spent;
        }
        if interactive && self.first_frame_ms.is_none() {
            self.first_frame_ms = Some(self.start.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn flush(&mut self) {
        if let Some((at, total)) = self.current.take() {
            self.frames.push((at, total.as_micros() as u64));
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

    pub fn write(&mut self) {
        self.flush();
        if let Err(err) = std::fs::write(&self.path, self.report()) {
            eprintln!("raabit: cannot write {}: {err}", self.path.display());
        }
    }
}

/// Runs `P` unchanged, timing it for [`Perf`].
pub struct PerfPlugin<P> {
    pub inner: P,
    pub perf: Perf,
}

impl<P: Plugin> Plugin for PerfPlugin<P> {
    fn manifest(&self) -> &PluginManifest {
        self.inner.manifest()
    }

    fn contributions(&self) -> Contributions {
        self.inner.contributions()
    }

    fn activate(&mut self, host: &mut dyn PluginHost) -> Result<(), PluginError> {
        self.inner.activate(host)
    }

    fn deactivate(&mut self, host: &mut dyn PluginHost) {
        self.inner.deactivate(host);
        self.perf.write();
    }

    fn execute(
        &mut self,
        command: &str,
        params: Value,
        host: &mut dyn PluginHost,
    ) -> CommandResult {
        self.inner.execute(command, params, host)
    }

    fn render_view(
        &mut self,
        view: &ViewContext,
        frame: &mut ViewFrame<'_>,
        host: &mut dyn PluginHost,
    ) {
        let began = Instant::now();
        self.inner.render_view(view, frame, host);
        self.perf.add(began.elapsed(), true);
    }

    fn status_widget(&self, id: &str) -> Option<archipel_sdk::api::StatusWidgetState> {
        self.inner.status_widget(id)
    }

    fn poll(&mut self, host: &mut dyn PluginHost) {
        self.perf.begin_frame();
        let began = Instant::now();
        self.inner.poll(host);
        self.perf.add(began.elapsed(), false);
        if self.perf.stop_requested() {
            host.dispatch(CMD_QUIT, Value::Null);
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
        let path = std::env::temp_dir().join(format!("raabit-perf-test-{}", std::process::id()));
        let mut perf = Perf::new(path.clone(), Instant::now());
        perf.begin_frame();
        perf.add(Duration::from_micros(5), false);
        perf.begin_frame();
        assert!(perf.report().contains("\"first_frame_ms\":null"));
        perf.add(Duration::from_micros(7), true);
        perf.write();
        let report = perf.report();
        assert!(!report.contains("null"));
        assert!(report.contains(",5]") && report.contains(",7]"));
        let _ = std::fs::remove_file(path);
    }
}
