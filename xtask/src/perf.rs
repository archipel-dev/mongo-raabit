//! `cargo xtask perf`: checks the speed budgets of `perf-budgets.toml` against the real app.
//!
//! Seeds a small database, starts `raabit --perf-report` under Xvfb, drives it with xdotool
//! (mouse only: welcome, connection dialog, connect, expand, open a 50-row table), reads the
//! report the app writes plus `/proc` for idle CPU and RSS, repeats that `--runs` times and
//! compares the medians to the budgets. Linux only; needs `Xvfb`, `xdotool` and a MongoDB
//! on `localhost:27017` (the address behind the welcome screen's "Connect to localhost").

use anyhow::{Context, Result, bail};
use mongodb::bson::{Document, doc};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DB: &str = "raabit_perf";
const COLLECTION: &str = "customers";
const IDLE_WINDOW: Duration = Duration::from_secs(5);
/// `USER_HZ` on Linux: `/proc/<pid>/stat` counts CPU time in 1/100 s.
const CLOCK_TICKS: f64 = 100.0;
const TIERS: [&str; 3] = ["free", "pro", "enterprise"];
const SCREEN: &str = "1280x800x24";

#[derive(Deserialize)]
struct Budgets {
    cold_start: ColdStart,
    frame: Frame,
    idle: Idle,
    memory: Memory,
    #[serde(default)]
    known_breach: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct ColdStart {
    max_ms: f64,
}
#[derive(Deserialize)]
struct Frame {
    max_ms: f64,
}
#[derive(Deserialize)]
struct Idle {
    max_cpu_percent: f64,
    max_repaints: f64,
}
#[derive(Deserialize)]
struct Memory {
    max_rss_mb: f64,
}

struct Options {
    runs: usize,
    uri: String,
    build: bool,
    bin: Option<PathBuf>,
    budgets: PathBuf,
    json: Option<PathBuf>,
    display: Option<String>,
}

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut o = Options {
            runs: 5,
            uri: std::env::var("MONGO_RAABIT_TEST_URI")
                .unwrap_or_else(|_| "mongodb://127.0.0.1:27017".into()),
            build: true,
            bin: None,
            budgets: "perf-budgets.toml".into(),
            json: None,
            display: None,
        };
        while let Some(arg) = args.next() {
            let mut value =
                |name: &str| args.next().with_context(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--runs" => o.runs = value("--runs")?.parse().context("--runs")?,
                "--uri" => o.uri = value("--uri")?,
                "--no-build" => o.build = false,
                "--bin" => o.bin = Some(value("--bin")?.into()),
                "--budgets" => o.budgets = value("--budgets")?.into(),
                "--json" => o.json = Some(value("--json")?.into()),
                "--display" => o.display = Some(value("--display")?),
                other => bail!("unknown argument `{other}`"),
            }
        }
        if o.runs == 0 {
            bail!("--runs must be at least 1");
        }
        Ok(o)
    }
}

pub fn run(args: impl Iterator<Item = String>) -> Result<()> {
    let o = Options::parse(args)?;
    if !cfg!(target_os = "linux") {
        bail!("cargo xtask perf needs Linux (Xvfb, xdotool, /proc)");
    }
    let budgets: Budgets = toml::from_str(&std::fs::read_to_string(&o.budgets)?)
        .with_context(|| format!("reading {}", o.budgets.display()))?;
    let bin = match &o.bin {
        Some(bin) => bin.clone(),
        None => PathBuf::from("target/release/raabit"),
    };
    if o.build && o.bin.is_none() {
        let status = Command::new("cargo")
            .args(["build", "--release", "-p", "raabit"])
            .status()?;
        if !status.success() {
            bail!("building raabit failed");
        }
    }
    let rt = tokio::runtime::Runtime::new()?;
    let client = rt.block_on(mongodb::Client::with_uri_str(&o.uri))?;
    rt.block_on(seed(&client)).context("seeding the server")?;

    let xvfb = match &o.display {
        Some(display) => Xvfb::existing(display),
        None => Xvfb::start()?,
    };
    let mut runs = Vec::new();
    for i in 0..o.runs {
        let before = rt.block_on(find_total(&client))?;
        let run = one_run(&bin, &xvfb.display, i)?;
        if rt.block_on(find_total(&client))? <= before {
            bail!("run {i}: the server saw no find; the driver did not reach the documents table");
        }
        eprintln!("run {}/{}: {}", i + 1, o.runs, run.summary());
        runs.push(run);
    }
    drop(xvfb);

    let checks = checks(&runs, &budgets);
    print_table(&checks, &budgets, o.runs);
    if let Some(path) = &o.json {
        let rows: Vec<_> = checks
            .iter()
            .map(|c| serde_json::json!({"check": c.key, "value": c.value, "max": c.max, "status": c.status(&budgets).0}))
            .collect();
        std::fs::write(path, serde_json::to_string_pretty(&rows)?)?;
    }
    let breaches = checks
        .iter()
        .filter(|c| c.status(&budgets).0 == "BREACH")
        .count();
    if breaches > 0 {
        bail!("{breaches} budget(s) breached");
    }
    Ok(())
}

/// 240 customers in `raabit_perf.customers`: the table shows the first 50.
async fn seed(client: &mongodb::Client) -> Result<()> {
    let db = client.database(DB);
    db.drop().await?;
    let docs: Vec<Document> = (0..240)
        .map(|i| {
            doc! {
                "name": format!("Customer {i:03}"),
                "email": format!("customer{i}@example.com"),
                "tier": TIERS[i % 3],
                "address": { "city": "Berlin", "zip": format!("{}", 10000 + i * 7) },
                "tags": ["alpha", "beta"],
                "active": i % 3 == 0,
            }
        })
        .collect();
    db.collection::<Document>(COLLECTION)
        .insert_many(docs)
        .await?;
    Ok(())
}

/// Server-wide count of `find` commands; proves the app opened the table.
async fn find_total(client: &mongodb::Client) -> Result<i64> {
    let status = client
        .database("admin")
        .run_command(doc! { "serverStatus": 1 })
        .await?;
    let find = status
        .get_document("metrics")?
        .get_document("commands")?
        .get_document("find")?;
    Ok(find
        .get_i64("total")
        .or_else(|_| find.get_i32("total").map(i64::from))?)
}

struct Xvfb {
    display: String,
    child: Option<Child>,
}

impl Xvfb {
    fn existing(display: &str) -> Self {
        Xvfb {
            display: display.into(),
            child: None,
        }
    }

    fn start() -> Result<Self> {
        let display = format!(":{}", 90 + std::process::id() % 100);
        let child = Command::new("Xvfb")
            .args([&display, "-screen", "0", SCREEN, "-nolisten", "tcp"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("starting Xvfb (apt install xvfb)")?;
        let xvfb = Xvfb {
            display,
            child: Some(child),
        };
        for _ in 0..50 {
            if xdo_on(&xvfb.display, &["getdisplaygeometry"]).is_ok() {
                return Ok(xvfb);
            }
            sleep(Duration::from_millis(100));
        }
        bail!("Xvfb did not come up on {}", xvfb.display)
    }
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn xdo_on(display: &str, args: &[&str]) -> Result<String> {
    let out = Command::new("xdotool")
        .args(args)
        .env("DISPLAY", display)
        .output()
        .context("running xdotool (apt install xdotool)")?;
    if !out.status.success() {
        bail!("xdotool {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// Where the driver clicks and hovers, in the 1280x800 window of `raabit --dark`. When a layout
/// change moves a target, the `find` check fails the run: re-read the positions from a
/// screenshot (`import -window root shot.png` under Xvfb) and update them here.
mod at {
    pub type Point = (u32, u32);
    pub const WELCOME_HOVER: [Point; 6] = [
        (700, 186),
        (785, 186),
        (785, 236),
        (850, 236),
        (700, 130),
        (400, 400),
    ];
    pub const TOOLBAR_CONNECT: Point = (193, 18);
    pub const DIALOG_HOVER: [Point; 6] = [
        (330, 196),
        (583, 276),
        (681, 276),
        (580, 334),
        (740, 182),
        (958, 671),
    ];
    /// The dialog's "Close" button.
    pub const DIALOG_CLOSE: Point = (392, 627);
    /// The welcome screen's "Connect to localhost".
    pub const CONNECT_LOCALHOST: Point = (785, 236);
    pub const TREE_HOVER: [Point; 5] = [(100, 115), (100, 139), (100, 163), (100, 187), (100, 211)];
    /// Expander arrow of the `raabit_perf` database row (the fourth database).
    pub const DB_ARROW: Point = (37, 211);
    /// The `customers` collection row below it.
    pub const COLLECTION: Point = (100, 235);
    pub const TABLE_HOVER: [Point; 6] = [
        (500, 385),
        (500, 441),
        (700, 500),
        (900, 560),
        (1100, 620),
        (600, 700),
    ];
    pub const TABLE_SCROLL: Point = (700, 500);
}

struct Driver<'a> {
    display: &'a str,
}

impl Driver<'_> {
    fn xdo(&self, args: &[&str]) -> Result<()> {
        xdo_on(self.display, args).map(drop)
    }

    fn click(&self, x: u32, y: u32) -> Result<()> {
        self.xdo(&["mousemove", &x.to_string(), &y.to_string(), "click", "1"])
    }

    fn double_click(&self, x: u32, y: u32) -> Result<()> {
        let (x, y) = (x.to_string(), y.to_string());
        self.xdo(&[
            "mousemove",
            &x,
            &y,
            "click",
            "--repeat",
            "2",
            "--delay",
            "80",
            "1",
        ])
    }

    /// Moves the pointer over `points`, one frame-worth of time apart.
    fn hover(&self, points: &[(u32, u32)]) -> Result<()> {
        for (x, y) in points {
            self.xdo(&["mousemove", &x.to_string(), &y.to_string()])?;
            sleep(Duration::from_millis(60));
        }
        Ok(())
    }

    fn scroll(&self, x: u32, y: u32, button: &str) -> Result<()> {
        self.xdo(&[
            "mousemove",
            &x.to_string(),
            &y.to_string(),
            "click",
            "--repeat",
            "6",
            "--delay",
            "60",
            button,
        ])
    }
}

#[derive(Default)]
struct Run {
    cold_start_ms: f64,
    /// screen → (p95 ms, max ms, frames)
    frames: BTreeMap<&'static str, (f64, f64, usize)>,
    idle_cpu_percent: f64,
    idle_repaints: usize,
    rss_mb: f64,
}

impl Run {
    fn summary(&self) -> String {
        let frames: Vec<String> = self
            .frames
            .iter()
            .map(|(k, (p95, ..))| format!("{k} {p95:.2}"))
            .collect();
        format!(
            "cold {:.0} ms, p95 [{} ms], idle {:.2} % / {} repaints, rss {:.0} MB",
            self.cold_start_ms,
            frames.join(", "),
            self.idle_cpu_percent,
            self.idle_repaints,
            self.rss_mb
        )
    }
}

fn one_run(bin: &Path, display: &str, index: usize) -> Result<Run> {
    let dir = std::env::temp_dir().join(format!("raabit-perf-{}-{index}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let report = dir.join("report.json");
    let spawn_ms = unix_ms();
    let mut child = Command::new(bin)
        .args(["--dark", "--perf-report"])
        .arg(&report)
        .env("DISPLAY", display)
        .env("HOME", &dir)
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .stdout(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", bin.display()))?;
    let result = drive(&mut child, display, &report, spawn_ms);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn drive(child: &mut Child, display: &str, report: &Path, spawn_ms: u128) -> Result<Run> {
    let d = Driver { display };
    for _ in 0..200 {
        if xdo_on(display, &["search", "--name", "Mongo Raabit"])
            .is_ok_and(|o| !o.trim().is_empty())
        {
            break;
        }
        sleep(Duration::from_millis(100));
    }
    sleep(Duration::from_millis(1500));
    let mut windows: Vec<(&'static str, u128, u128)> = Vec::new();
    let mut screen = |name: &'static str, start: u128| windows.push((name, start, unix_ms()));

    // Welcome: the window from process start, then the pointer over the welcome buttons.
    d.hover(&at::WELCOME_HOVER)?;
    screen("welcome", spawn_ms);

    // Connection dialog: open from the toolbar, move over its tabs and fields, close.
    let start = unix_ms();
    d.click(at::TOOLBAR_CONNECT.0, at::TOOLBAR_CONNECT.1)?;
    sleep(Duration::from_millis(600));
    d.hover(&at::DIALOG_HOVER)?;
    d.click(at::DIALOG_CLOSE.0, at::DIALOG_CLOSE.1)?;
    sleep(Duration::from_millis(400));
    screen("dialog", start);

    // Connect to localhost: the tree with its databases.
    let start = unix_ms();
    d.click(at::CONNECT_LOCALHOST.0, at::CONNECT_LOCALHOST.1)?;
    sleep(Duration::from_millis(2500));
    d.hover(&at::TREE_HOVER)?;
    screen("tree", start);

    // Expand `raabit_perf`, open `customers`: 50 rows. Hover, then scroll down and back.
    let start = unix_ms();
    d.click(at::DB_ARROW.0, at::DB_ARROW.1)?;
    sleep(Duration::from_millis(700));
    d.double_click(at::COLLECTION.0, at::COLLECTION.1)?;
    sleep(Duration::from_millis(2000));
    d.hover(&at::TABLE_HOVER)?;
    d.scroll(at::TABLE_SCROLL.0, at::TABLE_SCROLL.1, "5")?;
    d.scroll(at::TABLE_SCROLL.0, at::TABLE_SCROLL.1, "4")?;
    sleep(Duration::from_millis(300));
    screen("table", start);

    // Idle on the table: nothing moves for the window.
    sleep(Duration::from_millis(1500));
    let pid = child.id();
    let (idle_start, ticks0) = (unix_ms(), cpu_ticks(pid)?);
    sleep(IDLE_WINDOW);
    let (idle_end, ticks1) = (unix_ms(), cpu_ticks(pid)?);
    let rss_mb = rss_kb(pid)? / 1024.0;

    // Ask the app to write its report and close: the stop file plus a mouse move for a frame.
    std::fs::write(report.with_extension("json.stop"), "")?;
    d.hover(&[(640, 400), (650, 410)])?;
    wait_exit(child)?;
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report).context("the app wrote no report")?)?;
    let first_frame_ms = json["first_frame_ms"]
        .as_f64()
        .context("no first frame in the report")?;
    let start_ms = json["start_unix_ms"].as_f64().context("no start time")?;
    let frames: Vec<(f64, f64)> = json["frames"]
        .as_array()
        .context("no frames")?
        .iter()
        .filter_map(|f| Some((f[0].as_f64()?, f[1].as_f64()? / 1000.0)))
        .collect();

    let mut run = Run {
        cold_start_ms: (start_ms - spawn_ms as f64).max(0.0) + first_frame_ms,
        idle_cpu_percent: (ticks1 - ticks0) / CLOCK_TICKS / (idle_end - idle_start) as f64
            * 1000.0
            * 100.0,
        idle_repaints: frames
            .iter()
            .filter(|(t, _)| (idle_start as f64..=idle_end as f64).contains(t))
            .count(),
        rss_mb,
        ..Run::default()
    };
    for (name, start, end) in windows {
        let mut ms: Vec<f64> = frames
            .iter()
            .filter(|(t, _)| (start as f64..=end as f64).contains(t))
            .map(|(_, ms)| *ms)
            .collect();
        if ms.is_empty() {
            bail!("no frames recorded on the {name} screen");
        }
        ms.sort_by(f64::total_cmp);
        let p95 = ms[((ms.len() as f64 * 0.95).ceil() as usize).clamp(1, ms.len()) - 1];
        run.frames.insert(name, (p95, ms[ms.len() - 1], ms.len()));
    }
    Ok(run)
}

fn wait_exit(child: &mut Child) -> Result<()> {
    for _ in 0..100 {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        sleep(Duration::from_millis(100));
    }
    bail!("raabit did not close after the stop request")
}

/// utime + stime of `pid`, in clock ticks.
fn cpu_ticks(pid: u32) -> Result<f64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // After the `(comm)` field: state is field 3, utime 14, stime 15.
    let rest = stat.rsplit_once(')').context("bad /proc stat")?.1;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let field =
        |i: usize| -> Result<f64> { Ok(fields.get(i).context("short /proc stat")?.parse()?) };
    Ok(field(11)? + field(12)?)
}

fn rss_kb(pid: u32) -> Result<f64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    let line = status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .context("no VmRSS")?;
    Ok(line
        .split_whitespace()
        .nth(1)
        .context("bad VmRSS")?
        .parse()?)
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

struct Check {
    key: String,
    label: String,
    unit: &'static str,
    value: f64,
    max: f64,
    detail: String,
}

impl Check {
    fn status<'a>(&self, budgets: &'a Budgets) -> (&'static str, Option<&'a String>) {
        let known = budgets.known_breach.get(&self.key);
        if self.value <= self.max {
            ("ok", known)
        } else if known.is_some() {
            ("KNOWN BREACH", known)
        } else {
            ("BREACH", None)
        }
    }
}

fn checks(runs: &[Run], b: &Budgets) -> Vec<Check> {
    let med = |f: &dyn Fn(&Run) -> f64| median(runs.iter().map(f).collect());
    let mut out = vec![Check {
        key: "cold_start".into(),
        label: "cold start to first interactive frame".into(),
        unit: "ms",
        value: med(&|r| r.cold_start_ms),
        max: b.cold_start.max_ms,
        detail: String::new(),
    }];
    for screen in ["welcome", "dialog", "tree", "table"] {
        let worst = runs.iter().map(|r| r.frames[screen].1).fold(0.0, f64::max);
        let frames: usize = runs.iter().map(|r| r.frames[screen].2).sum();
        out.push(Check {
            key: format!("frame.{screen}"),
            label: format!("frame p95, {screen}"),
            unit: "ms",
            value: med(&|r| r.frames[screen].0),
            max: b.frame.max_ms,
            detail: format!("worst frame {worst:.2} ms over {frames} frames"),
        });
    }
    out.push(Check {
        key: "idle.cpu".into(),
        label: format!("idle CPU over {} s", IDLE_WINDOW.as_secs()),
        unit: "%",
        value: med(&|r| r.idle_cpu_percent),
        max: b.idle.max_cpu_percent,
        detail: String::new(),
    });
    out.push(Check {
        key: "idle.repaints".into(),
        label: format!("idle repaints over {} s", IDLE_WINDOW.as_secs()),
        unit: "frames",
        value: med(&|r| r.idle_repaints as f64),
        max: b.idle.max_repaints,
        detail: String::new(),
    });
    out.push(Check {
        key: "memory.rss".into(),
        label: "RSS, 50 documents shown".into(),
        unit: "MB",
        value: med(&|r| r.rss_mb),
        max: b.memory.max_rss_mb,
        detail: String::new(),
    });
    out
}

fn print_table(checks: &[Check], b: &Budgets, runs: usize) {
    println!("\nperf budgets (median of {runs} runs)\n");
    println!(
        "{:<14} {:<38} {:>10} {:>10}  status",
        "check", "what", "measured", "budget"
    );
    for c in checks {
        let (status, issue) = c.status(b);
        let issue = issue.map(|i| format!(" ({i})")).unwrap_or_default();
        println!(
            "{:<14} {:<38} {:>7.2} {:<2} {:>7.2} {:<2}  {status}{issue}{}",
            c.key,
            c.label,
            c.value,
            c.unit,
            c.max,
            c.unit,
            if c.detail.is_empty() {
                String::new()
            } else {
                format!("  [{}]", c.detail)
            }
        );
    }
}
