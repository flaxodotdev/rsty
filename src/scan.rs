use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use jwalk::{Parallelism, WalkDirGeneric};
use rayon::prelude::*;

use crate::model::{Item, Kind, Msg};
use crate::util::{alloc, home};

/// Items smaller than this are not worth a row.
const MIN_SHOW: u64 = 1 << 20;
const FOLDER_DEPTH: usize = 2;
const FOLDER_LIMIT: usize = 400;

#[derive(Clone)]
pub struct ScanConfig {
    pub roots: Vec<PathBuf>,
    pub min_large: u64,
    pub threads: usize,
}

pub struct ScanHandle {
    pub rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
}

impl Drop for ScanHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn start(cfg: ScanConfig) -> ScanHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    thread::spawn(move || Scanner::new(cfg, tx, c).run());
    ScanHandle { rx, cancel }
}

/// Per-entry state computed on jwalk's worker threads.
#[derive(Debug, Clone, Default)]
struct Info {
    size: u64,
    mtime: Option<SystemTime>,
    kind: Option<Kind>,
    skip: bool,
}

struct Sized {
    item: Item,
    files: u64,
}

struct Scanner {
    cfg: ScanConfig,
    tx: Sender<Msg>,
    cancel: Arc<AtomicBool>,
    jobs_tx: Sender<Sized>,
    jobs_rx: Receiver<Sized>,
    pending: usize,
    files: u64,
    bytes: u64,
    current: PathBuf,
    folders: HashMap<PathBuf, u64>,
    last_progress: Instant,
    last_folders: Instant,
}

impl Scanner {
    fn new(cfg: ScanConfig, tx: Sender<Msg>, cancel: Arc<AtomicBool>) -> Self {
        let (jobs_tx, jobs_rx) = mpsc::channel();
        Scanner {
            cfg,
            tx,
            cancel,
            jobs_tx,
            jobs_rx,
            pending: 0,
            files: 0,
            bytes: 0,
            current: PathBuf::new(),
            folders: HashMap::new(),
            last_progress: Instant::now(),
            last_folders: Instant::now(),
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn run(mut self) {
        let start = Instant::now();
        let (known, skip_dirs) = known_locations();

        let mut skip: HashSet<PathBuf> = skip_dirs.into_iter().collect();
        let mut ancestors: HashSet<PathBuf> = HashSet::new();
        for k in &known {
            skip.insert(k.path.clone());
        }
        for p in &skip {
            for a in p.ancestors().skip(1) {
                ancestors.insert(a.to_path_buf());
            }
        }
        let skip = Arc::new(skip);
        let ancestors = Arc::new(ancestors);

        for item in known {
            self.spawn_size(item);
        }

        let roots = self.cfg.roots.clone();
        for root in &roots {
            if self.cancelled() {
                return;
            }
            self.walk(root, skip.clone(), ancestors.clone());
        }

        while self.pending > 0 && !self.cancelled() {
            if let Ok(job) = self.jobs_rx.recv_timeout(Duration::from_millis(100)) { self.finish_job(job) }
            self.progress(false);
        }
        self.progress(true);
        self.send_folders();
        let _ = self.tx.send(Msg::Done(start.elapsed()));
    }

    fn walk(&mut self, root: &Path, skip: Arc<HashSet<PathBuf>>, ancestors: Arc<HashSet<PathBuf>>) {
        let min_large = self.cfg.min_large;
        let walker = WalkDirGeneric::<((), Info)>::new(root)
            .skip_hidden(false)
            .follow_links(false)
            .sort(false)
            .parallelism(Parallelism::RayonNewPool(self.cfg.threads))
            .process_read_dir(move |_depth, dir, _state, children| {
                let names: Vec<String> = children
                    .iter()
                    .filter_map(|c| c.as_ref().ok())
                    .map(|c| c.file_name.to_string_lossy().into_owned())
                    .collect();
                let has = |n: &str| names.iter().any(|x| x == n);
                for child in children.iter_mut() {
                    let Ok(e) = child else { continue };
                    if e.file_type.is_dir() {
                        let p = e.path();
                        if skip.contains(&p) {
                            e.read_children_path = None;
                            e.client_state.skip = true;
                        } else if !ancestors.contains(&p) {
                            if let Some(kind) = classify(dir, &e.file_name.to_string_lossy(), &has) {
                                e.read_children_path = None;
                                e.client_state.kind = Some(kind);
                            }
                        }
                    } else if e.file_type.is_file() {
                        if let Ok(md) = e.metadata() {
                            e.client_state.size = alloc(&md);
                            if e.client_state.size >= min_large {
                                e.client_state.mtime = md.modified().ok();
                            }
                        }
                    }
                }
            });

        for entry in walker {
            if self.cancelled() {
                return;
            }
            while let Ok(job) = self.jobs_rx.try_recv() {
                self.finish_job(job);
            }
            let Ok(e) = entry else { continue };
            let st = &e.client_state;
            if st.skip {
                continue;
            }
            if let Some(kind) = st.kind {
                self.spawn_size(Item::new(kind, e.path()));
                continue;
            }
            if e.file_type.is_file() {
                let size = st.size;
                let path = e.path();
                self.files += 1;
                self.bytes += size;
                self.attribute(&path, size, false);
                if size >= min_large {
                    let mut it = Item::new(Kind::LargeFile, path);
                    it.size = size;
                    it.mtime = st.mtime;
                    it.note = file_note(&it.path);
                    if self.tx.send(Msg::Item(it)).is_err() {
                        self.cancel.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            } else if e.file_type.is_dir() {
                self.current = e.path();
            }
            self.progress(false);
        }
    }

    fn spawn_size(&mut self, item: Item) {
        self.pending += 1;
        let tx = self.jobs_tx.clone();
        let cancel = self.cancel.clone();
        rayon::spawn(move || {
            let mut item = item;
            let (bytes, files) = if cancel.load(Ordering::Relaxed) {
                (0, 0)
            } else if item.clear_contents {
                // size of children only (e.g. Trash)
                let kids: Vec<PathBuf> = fs::read_dir(&item.path)
                    .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
                    .unwrap_or_default();
                kids.par_iter()
                    .map(|k| du(k, &cancel))
                    .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
            } else {
                du(&item.path, &cancel)
            };
            item.size = bytes;
            enrich(&mut item);
            let _ = tx.send(Sized { item, files });
        });
    }

    fn finish_job(&mut self, job: Sized) {
        self.pending -= 1;
        self.files += job.files;
        self.bytes += job.item.size;
        self.attribute(&job.item.path.clone(), job.item.size, true);
        if job.item.size >= MIN_SHOW && self.tx.send(Msg::Item(job.item)).is_err() {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Roll a size up into the top-level folders of whichever root contains it.
    fn attribute(&mut self, path: &Path, size: u64, is_dir: bool) {
        if size == 0 {
            return;
        }
        for root in &self.cfg.roots {
            let Ok(rel) = path.strip_prefix(root) else { continue };
            let comps: Vec<_> = rel.components().collect();
            let mut prefix = root.clone();
            for (d, c) in comps.iter().enumerate().take(FOLDER_DEPTH) {
                prefix.push(c);
                if d + 1 == comps.len() && !is_dir {
                    break;
                }
                *self.folders.entry(prefix.clone()).or_default() += size;
            }
            break;
        }
    }

    fn progress(&mut self, force: bool) {
        if !force && self.last_progress.elapsed() < Duration::from_millis(100) {
            return;
        }
        self.last_progress = Instant::now();
        let _ = self.tx.send(Msg::Progress {
            files: self.files,
            bytes: self.bytes,
            current: self.current.clone(),
            pending: self.pending,
        });
        if self.last_folders.elapsed() > Duration::from_millis(1500) {
            self.last_folders = Instant::now();
            self.send_folders();
        }
    }

    fn send_folders(&mut self) {
        let mut v: Vec<(&PathBuf, &u64)> = self.folders.iter().filter(|(_, s)| **s >= MIN_SHOW).collect();
        v.sort_unstable_by(|a, b| b.1.cmp(a.1));
        let items = v
            .into_iter()
            .take(FOLDER_LIMIT)
            .map(|(p, s)| {
                let mut it = Item::new(Kind::Folder, p.clone());
                it.size = *s;
                it
            })
            .collect();
        let _ = self.tx.send(Msg::Folders(items));
    }
}

/// Parallel `du`: (allocated bytes, file count). Never follows symlinks.
pub fn du(path: &Path, cancel: &AtomicBool) -> (u64, u64) {
    let Ok(md) = fs::symlink_metadata(path) else { return (0, 0) };
    if !md.is_dir() {
        return (alloc(&md), 1);
    }
    if cancel.load(Ordering::Relaxed) {
        return (0, 0);
    }
    let Ok(rd) = fs::read_dir(path) else { return (alloc(&md), 0) };
    let entries: Vec<fs::DirEntry> = rd.filter_map(|e| e.ok()).collect();
    let (b, f) = entries
        .par_iter()
        .map(|e| match e.file_type() {
            Ok(ft) if ft.is_dir() => du(&e.path(), cancel),
            Ok(_) => (e.metadata().map(|m| alloc(&m)).unwrap_or(0), 1),
            Err(_) => (0, 0),
        })
        .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    (b + alloc(&md), f)
}

/// Decide whether a directory is a well-known, regenerable artifact.
fn classify(parent: &Path, name: &str, has: &dyn Fn(&str) -> bool) -> Option<Kind> {
    let inside = |f: &str| parent.join(name).join(f).exists();
    Some(match name {
        "target"
            if has("Cargo.toml") || inside(".rustc_info.json") || inside("debug/.fingerprint") =>
        {
            Kind::RustTarget
        }
        "node_modules" => Kind::NodeModules,
        ".venv" | "venv" | "env" | ".env" if inside("pyvenv.cfg") => Kind::PyVenv,
        "__pycache__" | ".pytest_cache" | ".mypy_cache" | ".ruff_cache" | ".tox" | ".nox" => Kind::PyCache,
        ".next" | ".nuxt" | ".turbo" | ".parcel-cache" | ".svelte-kit" | ".angular" | ".expo" => Kind::JsBuild,
        ".gradle" => Kind::Gradle,
        "build" if has("build.gradle") || has("build.gradle.kts") => Kind::Gradle,
        ".build" if has("Package.swift") => Kind::SwiftBuild,
        ".zig-cache" | "zig-cache" => Kind::ZigCache,
        "_build" if has("mix.exs") => Kind::Elixir,
        "dist-newstyle" | ".stack-work" => Kind::Haskell,
        _ => return None,
    })
}

/// Fills in note + mtime. Runs on a worker thread after sizing.
fn enrich(item: &mut Item) {
    let p = item.path.clone();
    let mtime_of = |p: &Path| fs::symlink_metadata(p).and_then(|m| m.modified()).ok();
    match item.kind {
        Kind::RustTarget => {
            let candidates = [
                p.clone(),
                p.join("debug"),
                p.join("release"),
                p.join("debug/.fingerprint"),
                p.join("release/.fingerprint"),
                p.join(".rustc_info.json"),
            ];
            item.mtime = candidates.iter().filter_map(|c| mtime_of(c)).max();
            let mut profiles: Vec<String> = fs::read_dir(&p)
                .map(|r| {
                    r.filter_map(|e| e.ok())
                        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| !n.starts_with('.') && n != "tmp" && n != "package")
                        .collect()
                })
                .unwrap_or_default();
            profiles.sort();
            let name = p
                .parent()
                .and_then(cargo_name)
                .unwrap_or_else(|| "custom target dir".into());
            item.note = if profiles.is_empty() {
                name
            } else {
                format!("{name} · {}", profiles.join("+"))
            };
        }
        Kind::LargeFile | Kind::Folder => {}
        _ => {
            if item.mtime.is_none() {
                item.mtime = mtime_of(&p);
            }
            if item.note.is_empty() {
                let proj = p
                    .parent()
                    .and_then(|x| x.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                item.note = format!("{} in {proj}", item.kind.label());
            }
        }
    }
}

/// Reads `name = "..."` from [package] (or notes a workspace).
fn cargo_name(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let mut section = "";
    let mut workspace = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            section = l;
            workspace |= l == "[workspace]";
            continue;
        }
        if section == "[package]" {
            if let Some(rest) = l.strip_prefix("name") {
                let v = rest.trim_start().strip_prefix('=')?.trim().trim_matches('"');
                return Some(v.to_string());
            }
        }
    }
    let dir_name = dir.file_name()?.to_string_lossy().into_owned();
    Some(if workspace { format!("{dir_name} (workspace)") } else { dir_name })
}

fn file_note(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.contains("/.git/objects/pack/") {
        return "git pack".into();
    }
    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.starts_with("core.") || name == "core" {
        return "core dump".into();
    }
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "dmg" | "iso" | "pkg" | "img" => "disk image / installer",
        "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "7z" | "rar" | "zst" => "archive",
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" => "video",
        "wav" | "flac" | "aiff" => "audio",
        "log" => "log file",
        "vmdk" | "qcow2" | "vdi" | "vhd" | "vhdx" | "raw" => "VM disk",
        "ipsw" => "iOS firmware",
        "gguf" | "safetensors" | "ckpt" | "pt" | "pth" | "onnx" | "bin" => "model / blob",
        "sqlite" | "db" | "sqlite3" => "database",
        "psd" | "blend" => "project file",
        "app" | "exe" | "msi" => "application",
        _ => "",
    }
    .into()
}

/// Locations that are swept directly instead of discovered by the walk.
/// Returns (items to size, extra dirs the walk must not descend into).
fn known_locations() -> (Vec<Item>, Vec<PathBuf>) {
    let h = home();
    let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from).unwrap_or_else(|| h.join(".cargo"));
    let rustup_home = std::env::var_os("RUSTUP_HOME").map(PathBuf::from).unwrap_or_else(|| h.join(".rustup"));

    let mut items = Vec::new();
    let mut skip = vec![
        rustup_home.clone(),
        pj(h, "Library/Caches"),
        h.join(".cache"),
        // walking these is slow, triggers cloud downloads or macOS privacy prompts
        pj(h, "Library/CloudStorage"),
        pj(h, "Library/Mobile Documents"),
        pj(h, "Library/Containers"),
        pj(h, "Library/Group Containers"),
        PathBuf::from("/proc"),
        PathBuf::from("/sys"),
        PathBuf::from("/dev"),
        PathBuf::from("/System/Volumes"),
    ];

    fn add(items: &mut Vec<Item>, kind: Kind, path: PathBuf, note: &str) {
        if path.exists() {
            let mut it = Item::new(kind, path);
            it.note = note.into();
            items.push(it);
        }
    }

    // ── Rust ──
    add(&mut items, Kind::CargoCache, pj(&cargo_home, "registry/cache"), "downloaded .crate archives · re-fetched on demand");
    add(&mut items, Kind::CargoCache, pj(&cargo_home, "registry/src"), "unpacked crate sources · re-extracted on demand");
    add(&mut items, Kind::CargoCache, pj(&cargo_home, "registry/index"), "registry index · re-synced on demand");
    add(&mut items, Kind::CargoCache, pj(&cargo_home, "git/checkouts"), "git dependency checkouts");
    add(&mut items, Kind::CargoCache, pj(&cargo_home, "git/db"), "git dependency repos");
    add(&mut items, Kind::Rustup, rustup_home.join("downloads"), "leftover rustup downloads");
    add(&mut items, Kind::Rustup, rustup_home.join("tmp"), "rustup temp files");

    let default_tc = fs::read_to_string(rustup_home.join("settings.toml")).ok().and_then(|t| {
        t.lines()
            .find_map(|l| l.trim().strip_prefix("default_toolchain"))
            .map(|r| r.trim_start_matches([' ', '=']).trim().trim_matches('"').to_string())
    });
    if let Ok(rd) = fs::read_dir(rustup_home.join("toolchains")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_default = default_tc
                .as_deref()
                .is_some_and(|d| name == d || name.starts_with(&format!("{d}-")));
            let mut it = Item::new(Kind::Rustup, e.path());
            it.note = if is_default {
                format!("toolchain {name} ⚠ default")
            } else {
                format!("toolchain {name} · `rustup toolchain uninstall` equiv.")
            };
            it.protected = is_default;
            items.push(it);
        }
    }

    // ── per-app caches (each child separately) ──
    for dir in [pj(h, "Library/Caches"), h.join(".cache")] {
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let lower = name.to_lowercase();
                let (kind, note) = if lower.contains("sccache") {
                    (Kind::CargoCache, "sccache compile cache".to_string())
                } else if lower.contains("homebrew") {
                    (Kind::Cache, "Homebrew downloads · `brew cleanup` equiv.".into())
                } else {
                    (Kind::Cache, format!("app cache · {name}"))
                };
                let mut it = Item::new(kind, e.path());
                it.note = note;
                items.push(it);
            }
        }
    }

    // ── dev tool caches ──
    let caches: &[(&str, &str)] = &[
        ("Library/Developer/Xcode/DerivedData", "Xcode build products"),
        ("Library/Developer/Xcode/iOS DeviceSupport", "device symbols · re-copied on connect"),
        ("Library/Developer/CoreSimulator/Caches", "simulator caches"),
        ("Library/Logs", "application logs"),
        (".npm/_cacache", "npm cache"),
        (".bun/install/cache", "bun cache"),
        ("Library/pnpm/store", "pnpm store"),
        (".local/share/pnpm/store", "pnpm store"),
        (".yarn/berry/cache", "yarn cache"),
        (".gradle/caches", "gradle caches"),
        (".gradle/wrapper/dists", "gradle wrapper distributions"),
        (".m2/repository", "maven artifacts"),
        ("go/pkg/mod", "Go module cache"),
        (".conda/pkgs", "conda package cache"),
        (".nuget/packages", "NuGet packages"),
        (".pub-cache", "Dart/Flutter pub cache"),
    ];
    for (rel, note) in caches {
        add(&mut items, Kind::Cache, pj(h, rel), note);
    }

    // ── Windows ──
    #[cfg(windows)]
    {
        let win: &[(&str, &str)] = &[
            ("AppData/Local/npm-cache", "npm cache"),
            ("AppData/Local/pip/cache", "pip cache"),
            ("AppData/Local/Yarn/Cache", "yarn cache"),
            ("AppData/Local/pnpm/store", "pnpm store"),
            ("AppData/Local/go-build", "Go build cache"),
            ("AppData/Local/NuGet/v3-cache", "NuGet HTTP cache"),
            ("AppData/Local/Microsoft/Windows/INetCache", "Windows internet cache"),
            ("AppData/Local/CrashDumps", "crash dumps"),
            ("AppData/Local/D3DSCache", "DirectX shader cache"),
        ];
        for (rel, note) in win {
            add(&mut items, Kind::Cache, pj(h, rel), note);
        }
        let temp = pj(h, "AppData/Local/Temp");
        if temp.exists() {
            let mut it = Item::new(Kind::Cache, temp.clone());
            it.note = "Windows temp files · in-use files are skipped".into();
            it.clear_contents = true;
            items.push(it);
        }
        skip.push(pj(h, "AppData/Local/Packages"));
    }

    // ── trash (contents only) ──
    for t in [h.join(".Trash"), pj(h, ".local/share/Trash")] {
        if t.exists() {
            let mut it = Item::new(Kind::Trash, t.clone());
            it.note = "emptying the trash · permanent".into();
            it.clear_contents = true;
            items.push(it);
            skip.push(t);
        }
    }

    (items, skip)
}

/// Joins a `/`-separated relative path component by component (portable).
fn pj(base: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(base.to_path_buf(), |p, c| p.join(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rust_target_and_node_modules() {
        let tmp = std::env::temp_dir().join(format!("rsty-test-{}", std::process::id()));
        let proj = tmp.join("proj");
        fs::create_dir_all(pj(&proj, "target/debug")).unwrap();
        fs::create_dir_all(pj(&proj, "node_modules/x")).unwrap();
        fs::write(proj.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        fs::write(pj(&proj, "target/debug/blob"), vec![1u8; 2 << 20]).unwrap();
        fs::write(pj(&proj, "node_modules/x/a.js"), vec![1u8; 2 << 20]).unwrap();

        let h = start(ScanConfig { roots: vec![tmp.clone()], min_large: u64::MAX, threads: 2 });
        let mut found = Vec::new();
        for m in h.rx.iter() {
            match m {
                Msg::Item(i) if i.path.starts_with(&tmp) => found.push(i),
                Msg::Done(_) => break,
                _ => {}
            }
        }
        let t = found.iter().find(|i| i.kind == Kind::RustTarget).expect("target found");
        assert!(t.note.starts_with("demo"), "{}", t.note);
        assert!(t.size >= 2 << 20);
        assert!(found.iter().any(|i| i.kind == Kind::NodeModules));
        fs::remove_dir_all(&tmp).unwrap();
    }
}
