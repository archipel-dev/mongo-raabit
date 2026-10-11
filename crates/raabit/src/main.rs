//! `raabit`: Mongo Raabit as a standalone desktop app.
//!
//! A thin eframe shell around the plugin of the `mongo-raabit` crate: it installs the
//! kit theme, owns the plugin host and renders the plugin's app view in the window.
//!
//! ```text
//! raabit [--light | --dark]
//! ```

mod host;

use archipel_sdk::api::{Plugin, ViewContext, ViewFrame, ViewPlacement};
use archipel_sdk::kit::{self, Theme, egui};
use host::StandaloneHost;
use mongo_raabit::MongoRaabitPlugin;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeChoice {
    Light,
    Dark,
}

/// The theme forced on the command line, if any.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<ThemeChoice>, String> {
    let mut choice = None;
    for arg in args {
        match arg.as_str() {
            "--light" => choice = Some(ThemeChoice::Light),
            "--dark" => choice = Some(ThemeChoice::Dark),
            other => {
                return Err(format!(
                    "unknown argument `{other}`; usage: raabit [--light | --dark]"
                ));
            }
        }
    }
    Ok(choice)
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
}

impl RaabitApp {
    fn new(cc: &eframe::CreationContext<'_>, forced: Option<ThemeChoice>) -> Self {
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
    }
}

fn main() -> eframe::Result {
    let forced = match parse_args(std::env::args().skip(1)) {
        Ok(choice) => choice,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
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
        Box::new(move |cc| Ok(Box::new(RaabitApp::new(cc, forced)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_flags() {
        let args = |a: &[&str]| parse_args(a.iter().map(|s| s.to_string()));
        assert_eq!(args(&[]), Ok(None));
        assert_eq!(args(&["--light"]), Ok(Some(ThemeChoice::Light)));
        assert_eq!(args(&["--light", "--dark"]), Ok(Some(ThemeChoice::Dark)));
        assert!(args(&["--nope"]).is_err());
    }
}
