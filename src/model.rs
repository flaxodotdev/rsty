use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use ratatui::style::Color;

/// The tabs shown in the UI. Every item belongs to exactly one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Rust,
    Dev,
    Caches,
    Large,
    Folders,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Rust, Tab::Dev, Tab::Caches, Tab::Large, Tab::Folders];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn title(self) -> &'static str {
        match self {
            Tab::Rust => "Rust",
            Tab::Dev => "Dev junk",
            Tab::Caches => "Caches",
            Tab::Large => "Large files",
            Tab::Folders => "Big folders",
        }
    }

    pub fn headline(self) -> &'static str {
        match self {
            Tab::Rust => "Rust leftovers",
            Tab::Dev => "Build artifacts & dependencies",
            Tab::Caches => "Caches, logs & trash",
            Tab::Large => "Huge files",
            Tab::Folders => "Biggest folders",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Tab::Rust => "target/ dirs come back with `cargo build`; registry caches re-download on demand",
            Tab::Dev => "node_modules, venvs & build dirs: reinstall/rebuild to get them back",
            Tab::Caches => "apps rebuild caches on their own; emptying Trash is forever",
            Tab::Large => "NOT junk by definition: look before you delete",
            Tab::Folders => "for orientation: deleting a whole folder deletes everything inside",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Tab::Rust => Color::Rgb(247, 118, 54),
            Tab::Dev => Color::Rgb(130, 200, 110),
            Tab::Caches => Color::Rgb(110, 170, 240),
            Tab::Large => Color::Rgb(220, 120, 220),
            Tab::Folders => Color::Rgb(170, 170, 170),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Folder,
    RustTarget,
    CargoCache,
    Rustup,
    NodeModules,
    PyVenv,
    PyCache,
    JsBuild,
    Gradle,
    SwiftBuild,
    ZigCache,
    Elixir,
    Haskell,
    Cache,
    Trash,
    LargeFile,
}

impl Kind {
    pub fn tab(self) -> Tab {
        match self {
            Kind::RustTarget | Kind::CargoCache | Kind::Rustup => Tab::Rust,
            Kind::NodeModules
            | Kind::PyVenv
            | Kind::PyCache
            | Kind::JsBuild
            | Kind::Gradle
            | Kind::SwiftBuild
            | Kind::ZigCache
            | Kind::Elixir
            | Kind::Haskell => Tab::Dev,
            Kind::Cache | Kind::Trash => Tab::Caches,
            Kind::LargeFile => Tab::Large,
            Kind::Folder => Tab::Folders,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Folder => "folder",
            Kind::RustTarget => "target/",
            Kind::CargoCache => "cargo cache",
            Kind::Rustup => "rustup",
            Kind::NodeModules => "node_modules",
            Kind::PyVenv => "python venv",
            Kind::PyCache => "python cache",
            Kind::JsBuild => "js build cache",
            Kind::Gradle => "gradle",
            Kind::SwiftBuild => "swift .build",
            Kind::ZigCache => "zig cache",
            Kind::Elixir => "elixir _build",
            Kind::Haskell => "haskell build",
            Kind::Cache => "cache",
            Kind::Trash => "trash",
            Kind::LargeFile => "file",
        }
    }

    /// Things that a tool regenerates without losing anything you made.
    pub fn regenerable(self) -> bool {
        !matches!(self, Kind::Folder | Kind::LargeFile | Kind::Trash)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Deleting,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: u64,
    pub kind: Kind,
    pub path: PathBuf,
    pub size: u64,
    pub mtime: Option<SystemTime>,
    pub note: String,
    /// Delete the directory's contents but keep the directory (e.g. ~/.Trash).
    pub clear_contents: bool,
    /// Shown with a warning and skipped by bulk-marking (e.g. default toolchain).
    pub protected: bool,
    pub marked: bool,
    pub state: State,
}

impl Item {
    pub fn new(kind: Kind, path: PathBuf) -> Self {
        Item {
            id: next_id(),
            kind,
            path,
            size: 0,
            mtime: None,
            note: String::new(),
            clear_contents: false,
            protected: false,
            marked: false,
            state: State::Idle,
        }
    }

    pub fn age(&self) -> Option<Duration> {
        self.mtime.and_then(|t| SystemTime::now().duration_since(t).ok())
    }

    pub fn is_stale(&self, days: u64) -> bool {
        self.age().is_some_and(|a| a.as_secs() > days * 86_400)
    }
}

pub fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub enum Msg {
    Item(Item),
    Progress {
        files: u64,
        bytes: u64,
        current: PathBuf,
        pending: usize,
    },
    Folders(Vec<Item>),
    Done(Duration),
    Deleted {
        id: u64,
        result: Result<bool, String>, // Ok(true) = moved to trash, Ok(false) = deleted
    },
}
