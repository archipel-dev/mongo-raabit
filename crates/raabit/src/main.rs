//! `raabit`: Mongo Raabit as a standalone desktop app.
//!
//! A thin eframe shell around the plugin of the `mongo-raabit` crate: it installs the
//! kit theme, owns the plugin host and renders the plugin's app view in the window.
//!
//! ```text
//! raabit [--light | --dark] [--perf-report <file>]
//! ```

mod host;
mod perf;

use archipel_sdk::api::{Plugin, ViewContext, ViewFrame, ViewPlacement};
use archipel_sdk::kit::{self, Theme, egui};
use host::StandaloneHost;
use mongo_raabit::MongoRaabitPlugin;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeChoice {
    Light,
    Dark,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    /// The theme forced on the command line, if any.
    theme: Option<ThemeChoice>,
    /// Where to write the perf report (`--perf-report`, else `RAABIT_PERF`).
    perf_report: Option<PathBuf>,
}

const USAGE: &str = "usage: raabit [--light | --dark] [--perf-report <file>]";

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--light" => parsed.theme = Some(ThemeChoice::Light),
            "--dark" => parsed.theme = Some(ThemeChoice::Dark),
            "--perf-report" => {
                let file = args
                    .next()
                    .ok_or_else(|| format!("--perf-report needs a file; {USAGE}"))?;
                parsed.perf_report = Some(file.into());
            }
            other => return Err(format!("unknown argument `{other}`; {USAGE}")),
        }
    }
    if parsed.perf_report.is_none() {
        parsed.perf_report = std::env::var_os(perf::ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
    }
    Ok(parsed)
}

fn theme_of(choice: ThemeChoice) -> Theme {
    match choice {
        ThemeChoice::Light => Theme::light(),
        ThemeChoice::Dark => Theme::dark(),
    }
}

struct RaabitApp {
    plugin: MongoRaabitPlugin,
    host: StandaloneHost,
    view: ViewContext,
    forced: Option<ThemeChoice>,
    shown: ThemeChoice,
    /// The first frame only paints the window; the plugin state is built on the second so
    /// the window appears before any file or runtime setup.
    started: bool,
    perf: Option<perf::Perf>,
}

impl RaabitApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        forced: Option<ThemeChoice>,
        perf: Option<perf::Perf>,
    ) -> Self {
        let plugin = MongoRaabitPlugin::new();
        let manifest = plugin.manifest();
        let view = manifest.app.as_ref().map_or_else(
            || mongo_raabit::plugin::VIEW.to_owned(),
            |app| app.view.clone(),
        );
        let host = StandaloneHost::new(
            manifest.id.clone(),
            manifest.permissions.clone(),
            host::default_data_dir(),
        );
        let shown = forced.unwrap_or_else(|| system_choice(&cc.egui_ctx));
        kit::install(&cc.egui_ctx, theme_of(shown));
        Self {
            plugin,
            host,
            view: ViewContext {
                view,
                placement: ViewPlacement::App,
            },
            forced,
            shown,
            started: false,
            perf,
        }
    }
}

fn system_choice(ctx: &egui::Context) -> ThemeChoice {
    match ctx.system_theme() {
        Some(egui::Theme::Light) => ThemeChoice::Light,
        _ => ThemeChoice::Dark,
    }
}

impl eframe::App for RaabitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(perf) = &mut self.perf {
            perf.begin_frame();
        }
        let interactive = self.started;
        let wanted = self.forced.unwrap_or_else(|| system_choice(ctx));
        if wanted != self.shown {
            self.shown = wanted;
            kit::install(ctx, theme_of(wanted));
        }
        let colors = Theme::colors(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(colors.frame))
            .show(ctx, |ui| {
                if !self.started {
                    self.started = true;
                    ctx.request_repaint();
                    return;
                }
                self.plugin.poll(&mut self.host);
                self.plugin
                    .render_view(&self.view, &mut ViewFrame::new(ui), &mut self.host);
            });
        if std::mem::take(&mut self.host.repaint) {
            ctx.request_repaint();
        }
        if let Some(perf) = &mut self.perf {
            perf.end_frame(interactive);
            if perf.stop_requested() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(perf) = &self.perf {
            perf.write();
        }
    }
}

fn main() -> eframe::Result {
    let start = Instant::now();
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let forced = args.theme;
    let perf = args.perf_report.map(|path| perf::Perf::new(path, start));
    let mut perf = Some(perf);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Mongo Raabit")
            .with_app_id("dev.archipel.raabit")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Mongo Raabit",
        options,
        Box::new(move |cc| Ok(Box::new(RaabitApp::new(cc, forced, perf.take().flatten())))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_flags() {
        let args = |a: &[&str]| parse_args(a.iter().map(|s| s.to_string()));
        assert_eq!(args(&[]).unwrap().theme, None);
        assert_eq!(args(&["--light"]).unwrap().theme, Some(ThemeChoice::Light));
        assert_eq!(
            args(&["--light", "--dark"]).unwrap().theme,
            Some(ThemeChoice::Dark)
        );
        assert!(args(&["--nope"]).is_err());
    }

    #[test]
    fn parses_perf_report() {
        let args = |a: &[&str]| parse_args(a.iter().map(|s| s.to_string()));
        assert_eq!(
            args(&["--perf-report", "p.json"]).unwrap().perf_report,
            Some("p.json".into())
        );
        assert!(args(&["--perf-report"]).is_err());
    }
}
