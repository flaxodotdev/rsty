use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::delete;
use crate::model::{Item, Kind, Msg, State, Tab};
use crate::scan::{self, ScanConfig, ScanHandle};
use crate::ui;
use crate::util::fmt_size;

pub const STALE_DAYS: u64 = 30;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Size,
    Age,
    Path,
}

impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Sort::Size => "size",
            Sort::Age => "oldest",
            Sort::Path => "path",
        }
    }
    fn next(self) -> Sort {
        match self {
            Sort::Size => Sort::Age,
            Sort::Age => Sort::Path,
            Sort::Path => Sort::Size,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Confirm,
    Help,
}

pub struct Progress {
    pub files: u64,
    pub bytes: u64,
    pub current: PathBuf,
    pub pending: usize,
}

pub struct App {
    pub cfg: ScanConfig,
    pub dry_run: bool,
    pub items: Vec<Item>,
    pub tab: Tab,
    pub sel: [Option<u64>; 5],
    pub sort: Sort,
    pub mode: Mode,
    pub scanning: bool,
    pub progress: Progress,
    pub started: Instant,
    pub elapsed: Option<Duration>,
    pub freed: u64,
    pub trashed: u64,
    pub tick: u64,
    pub status: Option<(String, Instant)>,
    pub confirm_ids: Vec<u64>,
    pub quit: bool,
    scan: ScanHandle,
    del_tx: Sender<Msg>,
    del_rx: Receiver<Msg>,
}

impl App {
    pub fn new(cfg: ScanConfig, dry_run: bool) -> Self {
        let (del_tx, del_rx) = mpsc::channel();
        App {
            scan: scan::start(cfg.clone()),
            cfg,
            dry_run,
            items: Vec::new(),
            tab: Tab::Rust,
            sel: [None; 5],
            sort: Sort::Size,
            mode: Mode::Normal,
            scanning: true,
            progress: Progress { files: 0, bytes: 0, current: PathBuf::new(), pending: 0 },
            started: Instant::now(),
            elapsed: None,
            freed: 0,
            trashed: 0,
            tick: 0,
            status: None,
            confirm_ids: Vec::new(),
            quit: false,
            del_tx,
            del_rx,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            self.pump();
            terminal.draw(|f| ui::draw(f, self))?;
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(k) = event::read()? {
                    if k.kind == KeyEventKind::Press {
                        self.on_key(k);
                    }
                }
            }
            self.tick = self.tick.wrapping_add(1);
        }
        Ok(())
    }

    /// Drain scanner + deleter messages. Returns false once the scan is over.
    pub fn pump(&mut self) -> bool {
        let mut msgs: Vec<Msg> = self.scan.rx.try_iter().take(5000).collect();
        msgs.extend(self.del_rx.try_iter());
        for m in msgs {
            self.apply(m);
        }
        self.scanning
    }

    fn apply(&mut self, m: Msg) {
        match m {
            Msg::Item(it) => self.items.push(it),
            Msg::Progress { files, bytes, current, pending } => {
                self.progress = Progress { files, bytes, current, pending };
            }
            Msg::Folders(new) => self.merge_folders(new),
            Msg::Done(d) => {
                self.scanning = false;
                self.elapsed = Some(d);
            }
            Msg::Deleted { id, result } => self.on_deleted(id, result),
        }
    }

    fn merge_folders(&mut self, new: Vec<Item>) {
        let old: HashMap<PathBuf, Item> = self
            .items
            .iter()
            .filter(|i| i.kind == Kind::Folder)
            .map(|i| (i.path.clone(), i.clone()))
            .collect();
        let mut keep: Vec<Item> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for mut it in new {
            if let Some(o) = old.get(&it.path) {
                it.id = o.id;
                it.marked = o.marked;
                it.state = o.state.clone();
            }
            seen.insert(it.path.clone());
            keep.push(it);
        }
        // keep marked / in-flight folders even if they dropped out of the top list
        for (p, o) in old {
            if !seen.contains(&p) && (o.marked || o.state != State::Idle) {
                keep.push(o);
            }
        }
        self.items.retain(|i| i.kind != Kind::Folder);
        self.items.extend(keep);
    }

    fn on_deleted(&mut self, id: u64, result: Result<bool, String>) {
        let Some(idx) = self.items.iter().position(|i| i.id == id) else { return };
        match result {
            Ok(trashed) => {
                let it = self.items.remove(idx);
                if trashed {
                    self.trashed += it.size;
                } else {
                    self.freed += it.size;
                }
                if it.clear_contents {
                    return;
                }
                // drop everything that lived inside, shrink everything containing it
                self.items.retain(|o| !o.path.starts_with(&it.path));
                for o in &mut self.items {
                    if it.path.starts_with(&o.path) {
                        o.size = o.size.saturating_sub(it.size);
                    }
                }
            }
            Err(e) => {
                self.items[idx].state = State::Failed(e.clone());
                self.flash(format!("✗ {e}"));
            }
        }
    }

    pub fn flash(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), Instant::now()));
    }

    /// Indices into `items` for a tab, sorted.
    pub fn view(&self, tab: Tab) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.items.len()).filter(|&i| self.items[i].kind.tab() == tab).collect();
        let it = &self.items;
        match self.sort {
            Sort::Size => v.sort_by(|&a, &b| it[b].size.cmp(&it[a].size)),
            Sort::Age => v.sort_by(|&a, &b| match (it[a].mtime, it[b].mtime) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => it[b].size.cmp(&it[a].size),
            }),
            Sort::Path => v.sort_by(|&a, &b| it[a].path.cmp(&it[b].path)),
        }
        v
    }

    pub fn tab_total(&self, tab: Tab) -> (u64, usize) {
        let (s, n) = self
            .items
            .iter()
            .filter(|i| i.kind.tab() == tab)
            .fold((0, 0), |(s, n), i| (s + i.size, n + 1));
        // Folders nest (depth 1 and 2), so summing them double counts.
        if tab == Tab::Folders {
            (self.progress.bytes, n)
        } else {
            (s, n)
        }
    }

    #[cfg(test)]
    pub fn wait(&mut self) {
        while self.pump() {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn marked(&self) -> (u64, usize) {
        self.items.iter().filter(|i| i.marked).fold((0, 0), |(s, n), i| (s + i.size, n + 1))
    }

    /// Cursor position within the current view.
    pub fn cursor(&self, view: &[usize]) -> usize {
        self.sel[self.tab.index()]
            .and_then(|id| view.iter().position(|&i| self.items[i].id == id))
            .unwrap_or(0)
    }

    fn current_id(&self) -> Option<u64> {
        let view = self.view(self.tab);
        view.get(self.cursor(&view)).map(|&i| self.items[i].id)
    }

    fn move_cursor(&mut self, delta: isize) {
        let view = self.view(self.tab);
        if view.is_empty() {
            return;
        }
        let pos = self.cursor(&view) as isize;
        let new = (pos + delta).clamp(0, view.len() as isize - 1) as usize;
        self.sel[self.tab.index()] = Some(self.items[view[new]].id);
    }

    fn set_tab(&mut self, t: Tab) {
        self.tab = t;
    }

    fn on_key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Help => self.mode = Mode::Normal,
            Mode::Confirm => match k.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => self.start_delete(false),
                KeyCode::Char('t') | KeyCode::Char('T') => self.start_delete(true),
                _ => {
                    self.mode = Mode::Normal;
                    self.confirm_ids.clear();
                }
            },
            Mode::Normal => self.on_normal_key(k),
        }
    }

    fn on_normal_key(&mut self, k: KeyEvent) {
        let page = 15;
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.set_tab(Tab::ALL[(self.tab.index() + 1) % 5]);
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.set_tab(Tab::ALL[(self.tab.index() + 4) % 5]);
            }
            KeyCode::Char(c @ '1'..='5') => self.set_tab(Tab::ALL[c as usize - '1' as usize]),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::PageDown => self.move_cursor(page),
            KeyCode::PageUp => self.move_cursor(-page),
            KeyCode::Home | KeyCode::Char('g') => self.move_cursor(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_cursor(isize::MAX / 2),
            KeyCode::Char(' ') => {
                if let Some(id) = self.current_id() {
                    if let Some(it) = self.items.iter_mut().find(|i| i.id == id) {
                        if it.state != State::Deleting {
                            it.marked = !it.marked;
                        }
                    }
                    self.move_cursor(1);
                }
            }
            KeyCode::Char('a') => {
                let tab = self.tab;
                let all = self
                    .items
                    .iter()
                    .filter(|i| i.kind.tab() == tab && !i.protected)
                    .all(|i| i.marked);
                for i in self.items.iter_mut().filter(|i| i.kind.tab() == tab && !i.protected) {
                    i.marked = !all && i.state != State::Deleting;
                }
            }
            KeyCode::Char('u') => {
                for i in &mut self.items {
                    i.marked = false;
                }
            }
            KeyCode::Char('m') => {
                let tab = self.tab;
                let (mut n, mut s) = (0, 0);
                for i in self.items.iter_mut().filter(|i| i.kind.tab() == tab) {
                    if !i.protected && i.is_stale(STALE_DAYS) && i.state == State::Idle && !i.marked {
                        i.marked = true;
                        n += 1;
                        s += i.size;
                    }
                }
                self.flash(format!("marked {n} items untouched for {STALE_DAYS}+ days ({})", fmt_size(s)));
            }
            KeyCode::Char('s') => self.sort = self.sort.next(),
            KeyCode::Char('o') => self.reveal(),
            KeyCode::Char('r') => self.rescan(),
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => self.ask_delete(),
            _ => {}
        }
    }

    fn ask_delete(&mut self) {
        let mut ids: Vec<u64> = self.items.iter().filter(|i| i.marked).map(|i| i.id).collect();
        if ids.is_empty() {
            if let Some(id) = self.current_id() {
                ids.push(id);
            }
        }
        ids.retain(|id| self.items.iter().any(|i| i.id == *id && i.state != State::Deleting));
        if ids.is_empty() {
            self.flash("nothing to delete");
            return;
        }
        // Sort largest first for the dialog.
        ids.sort_by_key(|id| std::cmp::Reverse(self.items.iter().find(|i| i.id == *id).map_or(0, |i| i.size)));
        self.confirm_ids = ids;
        self.mode = Mode::Confirm;
    }

    pub fn confirm_items(&self) -> Vec<&Item> {
        self.confirm_ids
            .iter()
            .filter_map(|id| self.items.iter().find(|i| i.id == *id))
            .collect()
    }

    fn start_delete(&mut self, to_trash: bool) {
        self.mode = Mode::Normal;
        let mut targets: Vec<(u64, PathBuf, bool)> = self
            .confirm_items()
            .iter()
            .map(|i| (i.id, i.path.clone(), i.clear_contents))
            .collect();
        self.confirm_ids.clear();
        // If a parent and child are both chosen, deleting the parent covers the child.
        targets.sort_by(|a, b| a.1.cmp(&b.1));
        let mut kept: Vec<(u64, PathBuf, bool)> = Vec::new();
        for t in targets {
            if kept.iter().any(|k| !k.2 && t.1.starts_with(&k.1)) {
                if let Some(it) = self.items.iter_mut().find(|i| i.id == t.0) {
                    it.marked = false;
                }
                continue;
            }
            kept.push(t);
        }
        for (id, ..) in &kept {
            if let Some(it) = self.items.iter_mut().find(|i| i.id == *id) {
                it.state = State::Deleting;
                it.marked = false;
            }
        }
        let tx = self.del_tx.clone();
        let dry = self.dry_run;
        let roots = self.cfg.roots.clone();
        thread::spawn(move || {
            for (id, path, clear) in kept {
                let result = delete::remove(&path, clear, to_trash, dry, &roots);
                if tx.send(Msg::Deleted { id, result }).is_err() {
                    break;
                }
            }
        });
    }

    fn reveal(&mut self) {
        let Some(id) = self.current_id() else { return };
        let Some(it) = self.items.iter().find(|i| i.id == id) else { return };
        let path = it.path.clone();
        #[cfg(target_os = "macos")]
        let r = std::process::Command::new("open").arg("-R").arg(&path).spawn();
        #[cfg(windows)]
        let r = {
            let mut arg = std::ffi::OsString::from("/select,");
            arg.push(&path);
            std::process::Command::new("explorer").arg(arg).spawn()
        };
        #[cfg(not(any(target_os = "macos", windows)))]
        let r = std::process::Command::new("xdg-open")
            .arg(path.parent().unwrap_or(&path))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Err(e) = r {
            self.flash(format!("couldn't open file manager: {e}"));
        }
    }

    fn rescan(&mut self) {
        self.items.retain(|i| i.state == State::Deleting);
        self.scan = scan::start(self.cfg.clone());
        self.scanning = true;
        self.started = Instant::now();
        self.elapsed = None;
        self.progress = Progress { files: 0, bytes: 0, current: PathBuf::new(), pending: 0 };
        self.flash("rescanning…");
    }
}
