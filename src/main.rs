mod app;
mod delete;
mod model;
mod scan;
mod ui;
mod util;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Parser;

use crate::app::App;
use crate::model::Tab;
use crate::scan::ScanConfig;
use crate::util::{display_path, fmt_age, fmt_size, home, parse_size};

/// 🦀 rsty: find the junk eating your disk (Rust target dirs, cargo caches,
/// node_modules, app caches, huge files) and get rid of it.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Directories to scan (default: your home directory)
    paths: Vec<PathBuf>,

    /// Files at least this big show up under "Large files"
    #[arg(short = 'm', long, default_value = "100M", value_parser = parse_size)]
    min_size: u64,

    /// Pretend to delete: nothing on disk is touched
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Print a plain-text report instead of opening the TUI
    #[arg(short, long)]
    report: bool,

    /// Rows per category in --report
    #[arg(long, default_value_t = 10)]
    top: usize,

    /// Walker threads (default: CPU count)
    #[arg(short = 'j', long)]
    threads: Option<usize>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut roots = Vec::new();
    for p in if cli.paths.is_empty() { vec![home().to_path_buf()] } else { cli.paths } {
        // canonicalize() yields `\\?\C:\…` verbatim paths on Windows; absolute() doesn't.
        #[cfg(windows)]
        let resolved = std::path::absolute(&p);
        #[cfg(not(windows))]
        let resolved = p.canonicalize();
        match resolved {
            Ok(c) if c.is_dir() => roots.push(c),
            _ => bail!("not a directory: {}", p.display()),
        }
    }
    let threads = cli
        .threads
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));
    let cfg = ScanConfig { roots, min_large: cli.min_size, threads };

    if cli.report {
        return report(cfg, cli.top);
    }

    let mut terminal = ratatui::init();
    let result = App::new(cfg, cli.dry_run).run(&mut terminal);
    ratatui::restore();
    result
}

fn report(cfg: ScanConfig, top: usize) -> Result<()> {
    let mut app = App::new(cfg, true);
    eprint!("scanning…");
    while app.pump() {
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!(
        "\rscanned {} files ({}) in {:.1}s\n",
        app.progress.files,
        fmt_size(app.progress.bytes),
        app.elapsed.unwrap_or_default().as_secs_f32()
    );
    for tab in Tab::ALL {
        let (total, n) = app.tab_total(tab);
        println!("== {} · {} in {} items", tab.headline(), fmt_size(total), n);
        for &i in app.view(tab).iter().take(top) {
            let it = &app.items[i];
            let age = it.age().map(fmt_age).unwrap_or_else(|| "-".into());
            println!("  {:>10}  {:>5}  {}  {}", fmt_size(it.size), age, display_path(&it.path), it.note);
        }
        println!();
    }
    Ok(())
}
