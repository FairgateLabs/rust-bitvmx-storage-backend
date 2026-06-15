use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::{collections::BTreeMap, fs::File, io, io::Write, time::Duration};
use storage_backend::storage::{KeyValueStore, Storage};

#[derive(Clone)]
struct BrowserEntry {
    label: String,
    display_path: String,
    full_key: Option<String>,
    is_group: bool,
    child_count: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusPanel {
    Keys,
    Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    ExportInput,
}

struct App {
    all_keys: Vec<String>,
    prefix: String,
    entries: Vec<BrowserEntry>,
    selected: usize,
    selection_by_prefix: BTreeMap<String, usize>,
    value_scroll: u16,
    focus: FocusPanel,
    mode: Mode,
    export_file: String,
    status: String,
}

impl App {
    fn new(storage: &Storage) -> Result<Self, String> {
        let mut all_keys = storage.keys(None).map_err(|e| e.to_string())?;
        all_keys.sort();

        let mut app = Self {
            all_keys,
            prefix: String::new(),
            entries: Vec::new(),
            selected: 0,
            selection_by_prefix: BTreeMap::new(),
            value_scroll: 0,
            focus: FocusPanel::Keys,
            mode: Mode::Normal,
            export_file: String::new(),
            status: "Tab switch panel • e export • Keys: ↑/↓ select, Enter/→ open, ← parent • Value: ↑/↓ scroll • r refresh • q/Esc quit"
                .to_string(),
        };
        app.refresh_entries();
        Ok(app)
    }

    fn refresh_keys(&mut self, storage: &Storage) -> Result<(), String> {
        self.save_current_selection();
        self.all_keys = storage.keys(None).map_err(|e| e.to_string())?;
        self.all_keys.sort();
        self.refresh_entries();
        self.value_scroll = 0;
        self.status = "Database keys refreshed".to_string();
        Ok(())
    }

    fn refresh_entries(&mut self) {
        self.entries = build_entries(&self.all_keys, &self.prefix);
        self.selected = self
            .selection_by_prefix
            .get(&self.prefix)
            .copied()
            .unwrap_or(0)
            .min(self.entries.len().saturating_sub(1));
    }

    fn save_current_selection(&mut self) {
        self.selection_by_prefix
            .insert(self.prefix.clone(), self.selected);
    }

    fn selected_entry(&self) -> Option<&BrowserEntry> {
        self.entries.get(self.selected)
    }

    fn select_previous(&mut self) {
        if self.entries.is_empty() {
            return;
        }

        if self.selected == 0 {
            self.selected = self.entries.len() - 1;
        } else {
            self.selected -= 1;
        }
        self.save_current_selection();
        self.value_scroll = 0;
    }

    fn select_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }

        self.selected = (self.selected + 1) % self.entries.len();
        self.save_current_selection();
        self.value_scroll = 0;
    }

    fn scroll_value_up(&mut self) {
        self.value_scroll = self.value_scroll.saturating_sub(1);
    }

    fn scroll_value_down(&mut self) {
        self.value_scroll = self.value_scroll.saturating_add(1);
    }

    fn toggle_focus(&mut self) {
        if self.focus == FocusPanel::Keys
            && self
                .selected_entry()
                .map(|entry| entry.is_group)
                .unwrap_or(false)
        {
            self.status =
                "Groups have no value panel content; open the group or select a key".to_string();
            return;
        }

        self.focus = match self.focus {
            FocusPanel::Keys => FocusPanel::Value,
            FocusPanel::Value => FocusPanel::Keys,
        };
        self.status = match self.focus {
            FocusPanel::Keys => "Focused keys panel".to_string(),
            FocusPanel::Value => "Focused value panel; use ↑/↓ to scroll".to_string(),
        };
    }

    fn open_selected(&mut self) {
        let Some(entry) = self.selected_entry() else {
            self.status = "No key selected".to_string();
            return;
        };

        if entry.is_group {
            let next_prefix = entry.display_path.clone();
            self.save_current_selection();
            self.prefix = next_prefix;
            self.value_scroll = 0;
            self.refresh_entries();
            self.status = format!("Opened {}", self.prefix);
        }
    }

    fn start_export(&mut self) {
        if self.selected_entry().is_none() {
            self.status = "No key or group selected to export".to_string();
            return;
        }

        self.mode = Mode::ExportInput;
        self.export_file.clear();
        self.status = "Enter export file path, Enter to export, Esc to cancel".to_string();
    }

    fn cancel_export(&mut self) {
        self.mode = Mode::Normal;
        self.export_file.clear();
        self.status = "Export cancelled".to_string();
    }

    fn export_selected(&mut self, storage: &Storage) {
        let Some(entry) = self.selected_entry().cloned() else {
            self.status = "No key or group selected to export".to_string();
            self.mode = Mode::Normal;
            return;
        };

        let path = self.export_file.trim().to_string();
        if path.is_empty() {
            self.status = "Export file path cannot be empty".to_string();
            return;
        }

        let result = export_entry(storage, &self.all_keys, &entry, &path);
        self.mode = Mode::Normal;
        self.export_file.clear();
        self.status = match result {
            Ok(count) => format!(
                "Exported {count} entr{}",
                if count == 1 { "y" } else { "ies" }
            ),
            Err(e) => format!("Export failed: {e}"),
        };
    }

    fn go_parent(&mut self) {
        if self.prefix.is_empty() {
            return;
        }

        self.save_current_selection();
        let trimmed = self.prefix.trim_end_matches(is_key_separator);
        self.prefix = match trimmed.rfind(is_key_separator) {
            Some(index) => trimmed[..=index].to_string(),
            None => String::new(),
        };
        self.value_scroll = 0;
        self.refresh_entries();
        self.status = if self.prefix.is_empty() {
            "Back to root".to_string()
        } else {
            format!("Back to {}", self.prefix)
        };
    }
}

pub fn run_tui(storage: &Storage) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = match App::new(storage) {
        Ok(app) => app,
        Err(e) => {
            disable_raw_mode()?;
            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
            return Err(io::Error::new(io::ErrorKind::Other, e));
        }
    };

    let result = loop {
        terminal.draw(|frame| draw(frame, &app, storage))?;
        if app.mode == Mode::ExportInput {
            terminal.show_cursor()?;
        }

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                if app.mode == Mode::ExportInput {
                    match key.code {
                        KeyCode::Esc => app.cancel_export(),
                        KeyCode::Enter => app.export_selected(storage),
                        KeyCode::Backspace => {
                            app.export_file.pop();
                        }
                        KeyCode::Char(c) => app.export_file.push(c),
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                    KeyCode::Tab => app.toggle_focus(),
                    KeyCode::Char('e') => app.start_export(),
                    KeyCode::Up => match app.focus {
                        FocusPanel::Keys => app.select_previous(),
                        FocusPanel::Value => app.scroll_value_up(),
                    },
                    KeyCode::Down => match app.focus {
                        FocusPanel::Keys => app.select_next(),
                        FocusPanel::Value => app.scroll_value_down(),
                    },
                    KeyCode::Enter | KeyCode::Right if app.focus == FocusPanel::Keys => {
                        app.open_selected()
                    }
                    KeyCode::Left | KeyCode::Backspace if app.focus == FocusPanel::Keys => {
                        app.go_parent()
                    }
                    KeyCode::Char('r') => {
                        if let Err(e) = app.refresh_keys(storage) {
                            app.status = format!("Failed to refresh keys: {e}");
                        }
                    }
                    _ => {}
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn draw(frame: &mut Frame<'_>, app: &App, storage: &Storage) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(frame.area());

    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(chunks[0]);

    draw_key_panel(frame, app, panels[0]);
    draw_value_panel(frame, app, storage, panels[1]);

    let status = Paragraph::new(app.status.as_str())
        .block(Block::default().borders(Borders::ALL).title("Help"));
    frame.render_widget(status, chunks[1]);

    if app.mode == Mode::ExportInput {
        draw_export_popup(frame, app);
    }
}

fn draw_export_popup(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect_fixed_height(70, 10, frame.area());
    frame.render_widget(Clear, area);

    let selected = app
        .selected_entry()
        .map(|entry| {
            if entry.is_group {
                format!("Export group {}", entry.display_path)
            } else {
                format!("Export key {}", entry.display_path)
            }
        })
        .unwrap_or_else(|| "Export nothing".to_string());

    let block = Block::default().borders(Borders::ALL).title("Export");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(Paragraph::new(selected), chunks[0]);
    frame.render_widget(Paragraph::new("File path"), chunks[1]);

    let input = Paragraph::new(app.export_file.as_str())
        .style(Style::default().fg(Color::Green))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(input, chunks[2]);

    frame.render_widget(
        Paragraph::new("Enter: export  Esc: cancel")
            .alignment(Alignment::Left)
            .style(Style::default().fg(Color::Yellow)),
        chunks[3],
    );

    let max_cursor_offset = chunks[2].width.saturating_sub(3) as usize;
    let cursor_offset = app.export_file.chars().count().min(max_cursor_offset) as u16;
    frame.set_cursor_position((chunks[2].x + 1 + cursor_offset, chunks[2].y + 1));
}

fn centered_rect_fixed_height(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical_margin = area.height.saturating_sub(height) / 2;
    let popup_height = height.min(area.height);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(vertical_margin),
            Constraint::Length(popup_height),
            Constraint::Min(0),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn draw_key_panel(frame: &mut Frame<'_>, app: &App, area: ratatui::layout::Rect) {
    let subtree_total = app
        .all_keys
        .iter()
        .filter(|key| key.starts_with(&app.prefix))
        .count();
    let title = if app.prefix.is_empty() {
        format!("Keys: / ({subtree_total} total)")
    } else {
        format!("Keys: {} ({subtree_total} total)", app.prefix)
    };

    let items = if app.entries.is_empty() {
        vec![ListItem::new(Line::from("No keys found"))]
    } else {
        app.entries
            .iter()
            .map(|entry| {
                let icon = if entry.is_group { "▸" } else { " " };
                let count = if entry.is_group {
                    format!("  [{}]", entry.child_count)
                } else {
                    String::new()
                };
                ListItem::new(Line::from(vec![
                    Span::styled(icon, Style::default().fg(Color::Cyan)),
                    Span::raw(" "),
                    Span::raw(entry.label.clone()),
                    Span::styled(count, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect()
    };

    let mut state = ListState::default();
    if !app.entries.is_empty() {
        state.select(Some(app.selected));
    }

    let border_style = if app.focus == FocusPanel::Keys {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("➤ ");

    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_value_panel(
    frame: &mut Frame<'_>,
    app: &App,
    storage: &Storage,
    area: ratatui::layout::Rect,
) {
    let (title, body) = match app.selected_entry() {
        Some(entry) if entry.is_group => (
            format!("Group {}", entry.display_path),
            format!(
                "{} keys start with this prefix.\n\nPress Enter to explore this group.",
                entry.child_count
            ),
        ),
        Some(entry) => {
            let key = entry.full_key.as_deref().unwrap_or(&entry.display_path);
            (format!("Value: {key}"), value_for_key(storage, key))
        }
        None => ("Value".to_string(), "No key selected".to_string()),
    };

    let border_style = if app.focus == FocusPanel::Value {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let paragraph = Paragraph::new(body)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        )
        .scroll((app.value_scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn value_for_key(storage: &Storage, key: &str) -> String {
    match storage.get::<&str, serde_json::Value>(key, None) {
        Ok(Some(value)) => {
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
        }
        Ok(None) => "Key not found".to_string(),
        Err(e) => format!("Failed to read value: {e}"),
    }
}

fn export_entry(
    storage: &Storage,
    all_keys: &[String],
    entry: &BrowserEntry,
    path: &str,
) -> Result<usize, String> {
    let keys: Vec<&str> = if entry.is_group {
        all_keys
            .iter()
            .filter(|key| key.starts_with(&entry.display_path))
            .map(String::as_str)
            .collect()
    } else {
        vec![entry
            .full_key
            .as_deref()
            .unwrap_or(entry.display_path.as_str())]
    };

    let mut json_map = serde_json::Map::new();
    for key in &keys {
        let value = storage
            .get::<&str, serde_json::Value>(key, None)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Key not found: {key}"))?;
        json_map.insert((*key).to_string(), value);
    }

    let json = serde_json::Value::Object(json_map);
    let mut file = File::create(path).map_err(|e| e.to_string())?;
    file.write_all(
        serde_json::to_string_pretty(&json)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())?;

    Ok(keys.len())
}

fn is_key_separator(c: char) -> bool {
    c == '/' || c == ':'
}

fn split_next_group(rest: &str) -> Option<(&str, char)> {
    rest.char_indices()
        .find(|(_, c)| is_key_separator(*c))
        .map(|(index, separator)| (&rest[..index], separator))
}

fn build_entries(keys: &[String], prefix: &str) -> Vec<BrowserEntry> {
    let mut grouped: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    let mut direct = Vec::new();

    for key in keys.iter().filter(|key| key.starts_with(prefix)) {
        let rest = &key[prefix.len()..];
        if rest.is_empty() {
            direct.push(BrowserEntry {
                label: key.clone(),
                display_path: key.clone(),
                full_key: Some(key.clone()),
                is_group: false,
                child_count: 0,
            });
            continue;
        }

        if let Some((segment, separator)) = split_next_group(rest) {
            grouped
                .entry(format!("{segment}{separator}"))
                .or_default()
                .push(key);
        } else {
            direct.push(BrowserEntry {
                label: rest.to_string(),
                display_path: key.clone(),
                full_key: Some(key.clone()),
                is_group: false,
                child_count: 0,
            });
        }
    }

    let mut entries = Vec::new();
    for (segment, members) in grouped {
        let group_prefix = format!("{prefix}{segment}");
        if members.len() > 1 {
            entries.push(BrowserEntry {
                label: segment,
                display_path: group_prefix,
                full_key: None,
                is_group: true,
                child_count: members.len(),
            });
        } else if let Some(key) = members.first() {
            entries.push(BrowserEntry {
                label: key[prefix.len()..].to_string(),
                display_path: (*key).clone(),
                full_key: Some((*key).clone()),
                is_group: false,
                child_count: 0,
            });
        }
    }

    entries.extend(direct);
    entries.sort_by(|a, b| match (a.is_group, b.is_group) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.label.cmp(&b.label),
    });
    entries
}
