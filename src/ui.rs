use std::time::Duration;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
    TableState, Wrap,
};
use ratatui::Frame;

use crate::app::{App, Mode, STALE_DAYS};
use crate::model::{Item, State, Tab};
use crate::util::{display_path, fmt_age, fmt_count, fmt_size, shorten_left};

const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const DIM: Color = Color::Rgb(120, 120, 130);
const BORDER: Color = Color::Rgb(70, 70, 85);
const RUST: Color = Color::Rgb(247, 118, 54);

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, breakdown, tabs, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(2),
    ])
    .areas(f.area());

    draw_header(f, app, header);
    draw_breakdown(f, app, breakdown);
    draw_tabs(f, app, tabs);
    draw_table(f, app, body);
    draw_footer(f, app, footer);

    match app.mode {
        Mode::Confirm => draw_confirm(f, app),
        Mode::Help => draw_help(f),
        Mode::Normal => {}
    }
}

fn spinner(app: &App) -> &'static str {
    SPIN[(app.tick / 2) as usize % SPIN.len()]
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(BORDER));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let title = Line::from(vec![
        Span::styled(" 🦀 rsty ", Style::new().fg(Color::Black).bg(RUST).bold()),
        Span::styled("  disk junk hunter", Style::new().fg(DIM)),
    ]);

    let p = &app.progress;
    let mut right: Vec<Span> = if app.scanning {
        let secs = app.started.elapsed().as_secs_f32();
        let pending = if p.pending > 0 { format!(" · sizing {}", p.pending) } else { String::new() };
        vec![
            Span::styled(format!("{} ", spinner(app)), Style::new().fg(RUST)),
            Span::raw(format!(
                "scanning {} files · {} · {secs:.0}s{pending}  ",
                fmt_count(p.files),
                fmt_size(p.bytes)
            )),
        ]
    } else {
        let d = app.elapsed.unwrap_or_default();
        vec![
            Span::styled("✔ ", Style::new().fg(Color::Green)),
            Span::raw(format!(
                "scanned {} files ({}) in {:.1}s  ",
                fmt_count(p.files),
                fmt_size(p.bytes),
                d.as_secs_f32()
            )),
        ]
    };
    if app.freed > 0 || app.trashed > 0 {
        right.push(Span::styled(format!(" freed {} ", fmt_size(app.freed)), Style::new().fg(Color::Black).bg(Color::Green).bold()));
        if app.trashed > 0 {
            right.push(Span::styled(format!(" +{} trashed ", fmt_size(app.trashed)), Style::new().fg(DIM)));
        }
    }
    if app.dry_run {
        right.push(Span::styled(" DRY RUN ", Style::new().fg(Color::Black).bg(Color::Yellow).bold()));
    }

    let [l, r] = Layout::horizontal([Constraint::Length(28), Constraint::Fill(1)]).areas(inner);
    f.render_widget(Paragraph::new(title), l);
    let right_line = Line::from(right).right_aligned();
    f.render_widget(Paragraph::new(right_line), r);

    // Under the bar, while scanning: the directory being read.
    if app.scanning && !p.current.as_os_str().is_empty() && area.height >= 3 {
        let cur = shorten_left(&display_path(&p.current), (area.width as usize).saturating_sub(6));
        let y = area.y + area.height - 1;
        let w = (cur.chars().count() as u16 + 2).min(area.width.saturating_sub(4));
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {cur} "), Style::new().fg(DIM))),
            Rect { x: area.x + 2, y, width: w, height: 1 },
        );
    }
}

/// A stacked bar showing where the reclaimable space is.
fn draw_breakdown(f: &mut Frame, app: &App, area: Rect) {
    let cats = [Tab::Rust, Tab::Dev, Tab::Caches, Tab::Large];
    let totals: Vec<(Tab, u64)> = cats.iter().map(|&t| (t, app.tab_total(t).0)).collect();
    let sum: u64 = totals.iter().map(|t| t.1).sum();
    let width = area.width.saturating_sub(2) as usize;

    let mut bar = vec![Span::raw(" ")];
    if sum == 0 {
        bar.push(Span::styled("░".repeat(width), Style::new().fg(BORDER)));
    } else {
        let mut used = 0;
        for (i, (t, s)) in totals.iter().enumerate() {
            let mut w = ((*s as f64 / sum as f64) * width as f64).round() as usize;
            if i == totals.len() - 1 {
                w = width.saturating_sub(used);
            }
            w = w.min(width.saturating_sub(used));
            used += w;
            bar.push(Span::styled("█".repeat(w), Style::new().fg(t.color())));
        }
    }

    let mut legend = vec![
        Span::raw(" "),
        Span::styled(fmt_size(sum), Style::new().bold().fg(Color::White)),
        Span::styled(" reclaimable   ", Style::new().fg(DIM)),
    ];
    for (t, s) in &totals {
        legend.push(Span::styled("■ ", Style::new().fg(t.color())));
        legend.push(Span::raw(format!("{} ", t.title())));
        legend.push(Span::styled(format!("{}   ", fmt_size(*s)), Style::new().fg(DIM)));
    }
    f.render_widget(Paragraph::new(vec![Line::from(bar), Line::from(legend)]), area);
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::raw(" ")];
    for (i, t) in Tab::ALL.iter().enumerate() {
        let (size, n) = app.tab_total(*t);
        let label = format!(" {} {} {} ", i + 1, t.title(), if n > 0 { fmt_size(size) } else { "·".into() });
        let style = if *t == app.tab {
            Style::new().fg(Color::Black).bg(t.color()).bold()
        } else {
            Style::new().fg(t.color())
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn size_color(b: u64) -> Color {
    match b {
        b if b >= 10 << 30 => Color::Rgb(255, 90, 90),
        b if b >= 1 << 30 => Color::Rgb(255, 170, 70),
        b if b >= 100 << 20 => Color::Rgb(235, 220, 120),
        _ => Color::Rgb(190, 190, 200),
    }
}

fn age_color(d: Duration) -> Color {
    let days = d.as_secs() / 86_400;
    if days > 180 {
        Color::Rgb(255, 110, 110)
    } else if days > STALE_DAYS {
        Color::Rgb(235, 200, 100)
    } else {
        Color::Rgb(120, 200, 130)
    }
}

fn size_bar(size: u64, max: u64, width: usize) -> String {
    const PARTS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    let frac = size as f64 / max.max(1) as f64;
    let eighths = (frac * (width * 8) as f64).round() as usize;
    let mut s = "█".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) {
        s.push(PARTS[eighths % 8 - 1]);
    }
    if s.is_empty() && size > 0 {
        s.push('▏');
    }
    s
}

fn draw_table(f: &mut Frame, app: &mut App, area: Rect) {
    let tab = app.tab;
    let view = app.view(tab);
    let (total, n) = app.tab_total(tab);
    let stale: (u64, usize) = view
        .iter()
        .map(|&i| &app.items[i])
        .filter(|i| i.is_stale(STALE_DAYS) && !i.protected)
        .fold((0, 0), |(s, c), i| (s + i.size, c + 1));

    let mut title = vec![
        Span::styled(format!(" {} ", tab.headline()), Style::new().fg(tab.color()).bold()),
        Span::styled(format!("· {} in {} · sort: {} ", fmt_size(total), n, app.sort.label()), Style::new().fg(DIM)),
    ];
    if stale.1 > 0 && tab != Tab::Folders {
        title.push(Span::styled(
            format!("· {} stale ({}) ", stale.1, fmt_size(stale.0)),
            Style::new().fg(Color::Rgb(235, 200, 100)),
        ));
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(tab.color()).add_modifier(Modifier::DIM))
        .title(Line::from(title))
        .title_bottom(Line::from(Span::styled(format!(" {} ", tab.hint()), Style::new().fg(DIM))).right_aligned());

    if view.is_empty() {
        let msg = if app.scanning {
            format!("{} looking…", spinner(app))
        } else {
            "Nothing here. Squeaky clean ✨".into()
        };
        let inner = block.inner(area);
        f.render_widget(block, area);
        let y = inner.y + inner.height / 2;
        f.render_widget(
            Paragraph::new(Line::from(msg).centered()).fg(DIM),
            Rect { y, height: 1, ..inner },
        );
        return;
    }

    let inner_w = area.width.saturating_sub(2) as usize;
    let fixed = 2 + 10 + 11 + 6 + 5 + 2; // mark, size, bar, age, spacing, highlight
    let flex = inner_w.saturating_sub(fixed);
    let path_w = (flex * 62 / 100).max(10);
    let note_w = flex.saturating_sub(path_w).max(6);

    let max = view.iter().map(|&i| app.items[i].size).max().unwrap_or(1);
    let rows: Vec<Row> = view
        .iter()
        .map(|&i| row(app, &app.items[i], max, path_w, note_w))
        .collect();

    let header = Row::new(["", "SIZE", "", "AGE", "PATH", "WHAT"])
        .style(Style::new().fg(DIM).add_modifier(Modifier::BOLD));
    let widths = [
        Constraint::Length(2),
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Length(6),
        Constraint::Length(path_w as u16),
        Constraint::Fill(1),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(1)
        .row_highlight_style(Style::new().bg(Color::Rgb(45, 45, 60)).add_modifier(Modifier::BOLD))
        .highlight_symbol("▌");

    let cursor = app.cursor(&view);
    let mut state = TableState::default().with_selected(Some(cursor));
    f.render_stateful_widget(table, area, &mut state);

    let visible = area.height.saturating_sub(3) as usize;
    if view.len() > visible {
        let mut sb = ScrollbarState::new(view.len()).position(cursor);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(None).end_symbol(None),
            area.inner(ratatui::layout::Margin { vertical: 1, horizontal: 0 }),
            &mut sb,
        );
    }
}

fn row<'a>(app: &App, it: &Item, max: u64, path_w: usize, note_w: usize) -> Row<'a> {
    let color = it.kind.tab().color();
    let mark = match (&it.state, it.marked) {
        (State::Deleting, _) => Span::styled(spinner(app), Style::new().fg(Color::Red)),
        (_, true) => Span::styled("●", Style::new().fg(Color::Rgb(255, 90, 90)).bold()),
        _ => Span::styled("○", Style::new().fg(BORDER)),
    };
    let size = Line::from(Span::styled(fmt_size(it.size), Style::new().fg(size_color(it.size)).bold())).right_aligned();
    let bar = Span::styled(size_bar(it.size, max, 10), Style::new().fg(color));
    let age = match it.age() {
        Some(d) => Span::styled(fmt_age(d), Style::new().fg(age_color(d))),
        None => Span::styled("—", Style::new().fg(BORDER)),
    };
    let path = Span::raw(shorten_left(&display_path(&it.path), path_w));
    let note = match &it.state {
        State::Deleting => Span::styled("deleting…", Style::new().fg(Color::Red)),
        State::Failed(e) => Span::styled(shorten_left(&format!("✗ {e}"), note_w), Style::new().fg(Color::Red)),
        State::Idle if it.protected => Span::styled(it.note.clone(), Style::new().fg(Color::Yellow)),
        State::Idle => Span::styled(it.note.clone(), Style::new().fg(DIM)),
    };
    let mut r = Row::new(vec![
        Cell::from(mark),
        Cell::from(size),
        Cell::from(bar),
        Cell::from(age),
        Cell::from(path),
        Cell::from(note),
    ]);
    if it.marked {
        r = r.style(Style::new().bg(Color::Rgb(60, 30, 30)));
    }
    r
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let (ms, mn) = app.marked();
    let mut l1 = vec![Span::raw(" ")];
    if mn > 0 {
        l1.push(Span::styled(
            format!(" {mn} marked · {} ", fmt_size(ms)),
            Style::new().fg(Color::Black).bg(Color::Rgb(255, 90, 90)).bold(),
        ));
        l1.push(Span::raw("  press "));
        l1.push(Span::styled("d", Style::new().bold().fg(Color::White)));
        l1.push(Span::raw(" to reclaim it"));
    } else {
        l1.push(Span::styled("nothing marked · ␣ to mark, m for everything stale", Style::new().fg(DIM)));
    }
    if let Some((s, t)) = &app.status {
        if t.elapsed() < Duration::from_secs(4) {
            l1.push(Span::styled(format!("   {s}"), Style::new().fg(Color::Rgb(235, 200, 100))));
        }
    }

    let keys = [
        ("←→", "tabs"),
        ("↑↓", "move"),
        ("␣", "mark"),
        ("a", "all"),
        ("m", "stale"),
        ("u", "unmark"),
        ("d", "delete"),
        ("o", "reveal"),
        ("s", "sort"),
        ("r", "rescan"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut l2 = vec![Span::raw(" ")];
    for (k, v) in keys {
        l2.push(Span::styled(format!(" {k} "), Style::new().fg(Color::Black).bg(Color::Rgb(150, 150, 165))));
        l2.push(Span::styled(format!(" {v}  "), Style::new().fg(DIM)));
    }
    f.render_widget(Paragraph::new(vec![Line::from(l1), Line::from(l2)]), area);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn draw_confirm(f: &mut Frame, app: &App) {
    let items = app.confirm_items();
    let total: u64 = items.iter().map(|i| i.size).sum();
    let shown = 8;
    let h = (items.len().min(shown) as u16) + 11;
    let area = centered(f.area(), 90, h);
    f.render_widget(Clear, area);

    let inner_w = area.width.saturating_sub(16) as usize;
    let mut lines = vec![Line::raw("")];
    for it in items.iter().take(shown) {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:>10}  ", fmt_size(it.size)), Style::new().fg(size_color(it.size)).bold()),
            Span::raw(shorten_left(&display_path(&it.path), inner_w)),
        ]));
    }
    if items.len() > shown {
        lines.push(Line::styled(format!("  … and {} more", items.len() - shown), Style::new().fg(DIM)));
    }
    lines.push(Line::raw(""));

    let risky = items.iter().filter(|i| !i.kind.regenerable()).count();
    let protected = items.iter().filter(|i| i.protected).count();
    if protected > 0 {
        lines.push(Line::styled(
            "  ⚠ includes your DEFAULT toolchain: rustup will need to reinstall it",
            Style::new().fg(Color::Yellow),
        ));
    }
    if risky > 0 {
        lines.push(Line::styled(
            format!("  ⚠ {risky} of these are your files, not regenerable junk. Make sure!"),
            Style::new().fg(Color::Rgb(255, 110, 110)).bold(),
        ));
    } else {
        lines.push(Line::styled(
            "  ✓ all of these are regenerated by their tools when needed",
            Style::new().fg(Color::Green),
        ));
    }
    if app.dry_run {
        lines.push(Line::styled("  (dry run: nothing will actually be removed)", Style::new().fg(Color::Yellow)));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(" enter/y ", Style::new().fg(Color::Black).bg(Color::Rgb(255, 90, 90)).bold()),
        Span::raw(" delete permanently    "),
        Span::styled(" t ", Style::new().fg(Color::Black).bg(Color::Rgb(150, 150, 165)).bold()),
        Span::raw(" move to Trash    "),
        Span::styled(" esc ", Style::new().fg(Color::Black).bg(Color::Rgb(150, 150, 165)).bold()),
        Span::raw(" cancel"),
    ]));

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Rgb(255, 90, 90)))
        .title(Line::from(Span::styled(
            format!(" Reclaim {} from {} item{}? ", fmt_size(total), items.len(), if items.len() == 1 { "" } else { "s" }),
            Style::new().bold().fg(Color::White),
        )));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_help(f: &mut Frame) {
    let area = centered(f.area(), 78, 26);
    f.render_widget(Clear, area);
    let k = |key: &str, what: &str| {
        Line::from(vec![
            Span::styled(format!("  {key:<14}"), Style::new().fg(RUST).bold()),
            Span::raw(what.to_string()),
        ])
    };
    let h = |s: &str| Line::styled(format!(" {s}"), Style::new().fg(Color::White).bold());
    let lines = vec![
        Line::raw(""),
        h("Keys"),
        k("← → / tab / 1-5", "switch tabs"),
        k("↑ ↓ / j k", "move   (PgUp/PgDn, g/G for top/bottom)"),
        k("space", "mark / unmark the current row"),
        k("a", "mark everything in this tab (toggle)"),
        k("m", &format!("mark everything not touched in {STALE_DAYS}+ days")),
        k("u", "unmark everything"),
        k("d / del", "delete marked (or current) items, after confirming"),
        k("o", "reveal in Finder / file manager"),
        k("s", "sort by size → oldest → path"),
        k("r", "rescan"),
        k("q / esc", "quit"),
        Line::raw(""),
        h("What gets found"),
        k("Rust", "target/ dirs, ~/.cargo registry & git caches, rustup"),
        k("", "toolchains, sccache. Age = last time it was built."),
        k("Dev junk", "node_modules, venvs, __pycache__, .next, gradle, zig…"),
        k("Caches", "~/Library/Caches, ~/.cache, Xcode, npm, go, maven, Trash"),
        k("Large files", "any single file above --min-size (default 100 MiB)"),
        k("Big folders", "two levels deep under each scan root"),
        Line::raw(""),
        Line::styled("  press any key to close", Style::new().fg(DIM)),
    ];
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(RUST))
        .title(Span::styled(" 🦀 rsty help ", Style::new().bold()));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::ScanConfig;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Renders every tab into a fake terminal; run with --nocapture to look at it.
    #[test]
    fn renders_all_tabs() {
        let root = std::env::current_dir().unwrap();
        let mut app = App::new(ScanConfig { roots: vec![root], min_large: 1 << 20, threads: 2 }, true);
        app.wait();
        let mut term = Terminal::new(TestBackend::new(150, 30)).unwrap();
        for t in Tab::ALL {
            app.tab = t;
            term.draw(|f| draw(f, &mut app)).unwrap();
            if std::env::var("RSTY_SHOW").is_ok() {
                let buf = term.backend().buffer();
                for y in 0..buf.area.height {
                    let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
                    println!("{line}");
                }
            }
        }
        app.mode = Mode::Help;
        term.draw(|f| draw(f, &mut app)).unwrap();
        app.mode = Mode::Confirm;
        app.confirm_ids = app.items.iter().take(3).map(|i| i.id).collect();
        term.draw(|f| draw(f, &mut app)).unwrap();
        // tiny terminal must not panic
        let mut small = Terminal::new(TestBackend::new(30, 8)).unwrap();
        small.draw(|f| draw(f, &mut app)).unwrap();
    }
}
