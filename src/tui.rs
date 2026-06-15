use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::{collections::BTreeMap, io, time::Duration};
use storage_backend::storage::{KeyValueStore, Storage};

#[derive(Clone)]
struct BrowserEntry {
    label: String,
    display_path: String,
    full_key: Option<String>,
    is_group: bool,
    child_count: usize,
}

struct App {
    all_keys: Vec<String>,
    prefix: String,
    entries: Vec<BrowserEntry>,
    selected: usize,
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
            status: "↑/↓ select • Enter open group • ←/Backspace parent • r refresh • q/Esc quit"
                .to_string(),
        };
        app.refresh_entries();
        Ok(app)
    }

    fn refresh_keys(&mut self, storage: &Storage) -> Result<(), String> {
        self.all_keys = storage.keys(None).map_err(|e| e.to_string())?;
        self.all_keys.sort();
        self.refresh_entries();
        self.status = "Database keys refreshed".to_string();
        Ok(())
    }

    fn refresh_entries(&mut self) {
        self.entries = build_entries(&self.all_keys, &self.prefix);
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
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
    }

    fn select_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }

        self.selected = (self.selected + 1) % self.entries.len();
    }

    fn open_selected(&mut self) {
        let Some(entry) = self.selected_entry() else {
            self.status = "No key selected".to_string();
            return;
        };

        if entry.is_group {
            self.prefix = entry.display_path.clone();
            self.selected = 0;
            self.refresh_entries();
            self.status = format!("Opened {}", self.prefix);
        }
    }

    fn go_parent(&mut self) {
        if self.prefix.is_empty() {
            return;
        }

        let trimmed = self.prefix.trim_end_matches(is_key_separator);
        self.prefix = match trimmed.rfind(is_key_separator) {
            Some(index) => trimmed[..=index].to_string(),
            None => String::new(),
        };
        self.selected = 0;
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

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                    KeyCode::Up => app.select_previous(),
                    KeyCode::Down => app.select_next(),
                    KeyCode::Enter | KeyCode::Right => app.open_selected(),
                    KeyCode::Left | KeyCode::Backspace => app.go_parent(),
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

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
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

    let paragraph = Paragraph::new(body)
        .block(Block::default().borders(Borders::ALL).title(title))
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
