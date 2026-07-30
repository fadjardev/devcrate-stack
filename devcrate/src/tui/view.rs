//! Drawing. Reads the app, writes to a frame, decides nothing.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
    Tabs, Wrap,
};

use crate::status::{ServiceStatus, State};
use crate::tui::app::{App, Modal, PhpPurpose, Tab};
use crate::tui::logs;

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;

pub fn render(frame: &mut Frame, app: &App) {
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(3), Constraint::Length(2)])
            .areas(frame.area());

    render_header(frame, app, header);
    match app.tab {
        Tab::Services => render_services(frame, app, body),
        Tab::Sites => render_sites(frame, app, body),
        Tab::Logs => render_logs(frame, app, body),
    }
    render_footer(frame, app, footer);

    if let Some(modal) = &app.modal {
        render_modal(frame, app, modal);
    }
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let [tabs_area, root_area] =
        Layout::horizontal([Constraint::Min(30), Constraint::Length(48)]).areas(area);

    let titles: Vec<Line> = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, tab)| Line::from(format!(" {} {} ", i + 1, tab.title())))
        .collect();
    let selected = Tab::ALL.iter().position(|tab| *tab == app.tab).unwrap_or(0);
    frame.render_widget(
        Tabs::new(titles)
            .select(selected)
            .highlight_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
            .divider("")
            .block(bordered("devcrate")),
        tabs_area,
    );

    let php = app
        .report
        .as_ref()
        .and_then(|report| report.cli_php.clone())
        .unwrap_or_else(|| "not set".into());
    let root = app.stack.root.display().to_string();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(root, Style::new().fg(MUTED)),
            Span::raw("   CLI PHP "),
            Span::styled(php, Style::new().fg(ACCENT)),
        ]))
        .alignment(Alignment::Right)
        .block(bordered("")),
        root_area,
    );
}

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

fn render_services(frame: &mut Frame, app: &App, area: Rect) {
    let Some(report) = &app.report else {
        frame.render_widget(
            Paragraph::new("Scanning...").block(bordered("Services")),
            area,
        );
        return;
    };

    let holders: Vec<String> = report
        .services
        .iter()
        .flat_map(|service| service.ports.iter())
        .filter_map(|port| {
            port.holder.as_ref().map(|who| format!("port {} is held by {who}", port.port))
        })
        .collect();

    let [table_area, notice_area] = if holders.is_empty() {
        [area, Rect::ZERO]
    } else {
        Layout::vertical([Constraint::Min(3), Constraint::Length(holders.len() as u16 + 2)])
            .areas(area)
    };

    let rows: Vec<Row> = report
        .services
        .iter()
        .map(|service| service_row(app, service))
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Length(14),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Min(20),
    ];
    let header = Row::new(["SERVICE", "STATE", "PORTS", "UPTIME", "PIDS", "PATH"])
        .style(Style::new().fg(MUTED).add_modifier(Modifier::BOLD));

    let mut state = TableState::default().with_selected(Some(app.service_row));
    frame.render_stateful_widget(
        Table::new(rows, widths)
            .header(header)
            .block(bordered("Services"))
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        table_area,
        &mut state,
    );

    if !holders.is_empty() {
        let lines: Vec<Line> =
            holders.into_iter().map(|line| Line::from(line.fg(Color::Red))).collect();
        frame.render_widget(
            Paragraph::new(lines).block(bordered("Ports held by something else")),
            notice_area,
        );
    }
}

fn service_row<'a>(app: &App, service: &'a ServiceStatus) -> Row<'a> {
    let crashed = app.crashed(&service.id);
    let (label, style) = if crashed {
        ("crashed".to_string(), Style::new().fg(Color::Red).add_modifier(Modifier::BOLD))
    } else {
        (service.state.as_str().to_string(), Style::new().fg(state_color(service.state)))
    };

    let ports = if service.ports.is_empty() {
        "-".to_string()
    } else {
        service
            .ports
            .iter()
            .map(|port| {
                if port.listening {
                    port.port.to_string()
                } else {
                    format!("({})", port.port)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };

    let pids = match service.pids.split_first() {
        None => "-".to_string(),
        Some((first, [])) => first.to_string(),
        Some((first, rest)) => format!("{first} +{}", rest.len()),
    };

    Row::new(vec![
        Cell::from(service.name.clone()),
        Cell::from(label).style(style),
        Cell::from(ports),
        Cell::from(crate::status::format_uptime(service.uptime_secs)),
        Cell::from(pids),
        Cell::from(service.path.clone()).style(Style::new().fg(MUTED)),
    ])
}

fn state_color(state: State) -> Color {
    match state {
        State::Up => Color::Green,
        State::Starting => Color::Yellow,
        State::PortBusy => Color::Red,
        State::Stopped | State::Absent => Color::DarkGray,
    }
}

// ---------------------------------------------------------------------------
// Sites
// ---------------------------------------------------------------------------

fn render_sites(frame: &mut Frame, app: &App, area: Rect) {
    if app.sites.is_empty() {
        frame.render_widget(
            Paragraph::new("No vhosts yet. Press n to create one.")
                .block(bordered("Sites")),
            area,
        );
        return;
    }

    let rows: Vec<Row> = app
        .sites
        .iter()
        .map(|site| {
            let php = app
                .php_for_port(site.fastcgi_port)
                .map(str::to_string)
                .or_else(|| site.fastcgi_port.map(|port| format!("port {port}")))
                .unwrap_or_else(|| "no fastcgi_pass".into());
            let root = site.root.clone().unwrap_or_else(|| "no root directive".into());
            Row::new(vec![
                Cell::from(format!("https://{}", site.host)),
                Cell::from(php),
                Cell::from(root).style(Style::new().fg(MUTED)),
            ])
        })
        .collect();

    let mut state = TableState::default().with_selected(Some(app.site_row));
    frame.render_stateful_widget(
        Table::new(
            rows,
            [Constraint::Min(24), Constraint::Length(12), Constraint::Min(20)],
        )
        .header(
            Row::new(["VHOST", "PHP", "ROOT"])
                .style(Style::new().fg(MUTED).add_modifier(Modifier::BOLD)),
        )
        .block(bordered(&format!("Sites ({})", app.sites.len())))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        area,
        &mut state,
    );
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

fn render_logs(frame: &mut Frame, app: &App, area: Rect) {
    let [picker_area, tail_area] =
        Layout::horizontal([Constraint::Length(34), Constraint::Min(30)]).areas(area);

    let items: Vec<ListItem> = app
        .logs
        .iter()
        .map(|file| {
            ListItem::new(Line::from(vec![
                Span::raw(file.label.clone()),
                Span::styled(
                    format!("  {}", logs::human_size(file.size)),
                    Style::new().fg(MUTED),
                ),
            ]))
        })
        .collect();

    let mut state = ListState::default().with_selected(Some(app.log_row));
    frame.render_stateful_widget(
        List::new(items)
            .block(bordered("Files"))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        picker_area,
        &mut state,
    );

    let title = match (&app.log, app.follow) {
        (None, _) => "Log".to_string(),
        (Some(view), true) => format!("{}  [following]", view.path.display()),
        (Some(view), false) => {
            format!("{}  [paused, {} lines up]", view.path.display(), app.log_scroll)
        }
    };

    let Some(view) = &app.log else {
        let message = if app.logs.is_empty() {
            "No log files yet. They appear once the services have run."
        } else {
            "Reading..."
        };
        frame.render_widget(Paragraph::new(message).block(bordered(&title)), tail_area);
        return;
    };

    // Show the window ending `log_scroll` lines above the last line.
    let height = tail_area.height.saturating_sub(2) as usize;
    let end = view.lines.len().saturating_sub(app.log_scroll);
    let start = end.saturating_sub(height);
    let mut lines: Vec<Line> =
        view.lines[start..end].iter().map(|line| Line::from(line.as_str())).collect();
    if start == 0 && view.truncated {
        lines.insert(0, Line::from("(earlier lines not shown)".fg(MUTED)));
    }
    if lines.is_empty() {
        lines.push(Line::from("(empty)".fg(MUTED)));
    }

    frame.render_widget(Paragraph::new(lines).block(bordered(&title)), tail_area);
}

// ---------------------------------------------------------------------------
// Footer and modals
// ---------------------------------------------------------------------------

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let [note_area, keys_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    let note = match (&app.busy, &app.note) {
        (Some(busy), _) => Line::from(vec![
            Span::styled(" working ", Style::new().bg(Color::Yellow).fg(Color::Black)),
            Span::raw(format!(" {busy}...")),
        ]),
        (None, Some(note)) if !note.is_stale() => {
            let colour = if note.failed { Color::Red } else { Color::Green };
            Line::from(Span::styled(format!(" {}", note.text), Style::new().fg(colour)))
        }
        _ => Line::from(Span::styled(" ready".to_string(), Style::new().fg(MUTED))),
    };
    frame.render_widget(Paragraph::new(note), note_area);

    let keys = match app.tab {
        Tab::Services => {
            "s start  x stop  t restart  (SHIFT: whole stack)  u CLI PHP  ? help  q quit"
        }
        Tab::Sites => "n new  p PHP version  d delete  ? help  q quit",
        Tab::Logs => "f follow  PgUp/PgDn scroll  Home/End  ? help  q quit",
    };
    frame.render_widget(
        Paragraph::new(Line::from(format!(" {keys}").fg(MUTED))),
        keys_area,
    );
}

fn render_modal(frame: &mut Frame, app: &App, modal: &Modal) {
    match modal {
        Modal::Help => popup(frame, "Keys", help_text(), 64, 20),
        Modal::Output { title, lines, failed } => {
            let colour = if *failed { Color::Red } else { Color::Reset };
            let text: Vec<Line> = lines
                .iter()
                .map(|line| Line::from(Span::styled(line.clone(), Style::new().fg(colour))))
                .collect();
            let height = (text.len() + 4).min(24) as u16;
            popup(frame, title, Text::from(text), 80, height);
        }
        Modal::Confirm { question, .. } => {
            let mut text = Text::from(question.as_str());
            text.push_line(Line::from(""));
            text.push_line(Line::from("y confirm     any other key cancels".fg(MUTED)));
            popup(frame, "Confirm", text, 64, 9);
        }
        Modal::NewSite { host } => {
            let mut text = Text::from(Line::from(vec![
                Span::raw("Hostname:  "),
                Span::styled(host.clone(), Style::new().fg(ACCENT)),
                Span::styled("_", Style::new().add_modifier(Modifier::SLOW_BLINK)),
            ]));
            text.push_line(Line::from(""));
            text.push_line(Line::from(
                "Enter chooses the PHP version next.  Esc cancels.".fg(MUTED),
            ));
            text.push_line(Line::from(
                "The hosts-file entry stays manual -- it needs elevation.".fg(MUTED),
            ));
            popup(frame, "New vhost", text, 62, 9);
        }
        Modal::PhpPicker { purpose, index } => {
            let versions = app.php_versions();
            let title = match purpose {
                PhpPurpose::Cli => "Switch the CLI PHP version".to_string(),
                PhpPurpose::Site(host) => format!("Serve {host} with"),
                PhpPurpose::NewSite(host) => format!("Create {host} with"),
            };
            let items: Vec<ListItem> = versions
                .iter()
                .map(|(id, name)| ListItem::new(format!("{name}   ({id})")))
                .collect();
            let height = (items.len() + 2).max(3) as u16;
            let area = centred(frame.area(), 46, height);
            frame.render_widget(Clear, area);
            let mut state = ListState::default().with_selected(Some(*index));
            frame.render_stateful_widget(
                List::new(items)
                    .block(bordered(&title))
                    .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
                area,
                &mut state,
            );
        }
        Modal::InstallPicker { index } => {
            let items: Vec<ListItem> = crate::tui::app::INSTALL_OPTIONS
                .iter()
                .map(|(_, label, _)| ListItem::new(*label))
                .collect();
            let height = (items.len() + 2).max(3) as u16;
            let area = centred(frame.area(), 46, height);
            frame.render_widget(Clear, area);
            let mut state = ListState::default().with_selected(Some(*index));
            frame.render_stateful_widget(
                List::new(items)
                    .block(bordered("Install Service / Runtime"))
                    .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
                area,
                &mut state,
            );
        }
    }
}

fn help_text() -> Text<'static> {
    Text::from(vec![
        Line::from("Anywhere".fg(ACCENT)),
        Line::from("  1 2 3 / Tab   switch pane        r   rescan now"),
        Line::from("  up down j k   move                q   quit"),
        Line::from(""),
        Line::from("Services".fg(ACCENT)),
        Line::from("  s / x / t     start / stop / restart the selected service"),
        Line::from("  S / X / T     ...the whole stack, in dependency order"),
        Line::from("  i             install service or runtime (Node, Bun, PHP, etc)"),
        Line::from("  u             repoint php\\current (the CLI version)"),
        Line::from(""),
        Line::from("Sites".fg(ACCENT)),
        Line::from("  n             new vhost           d   delete the conf"),
        Line::from("  p             change PHP version"),
        Line::from(""),
        Line::from("Logs".fg(ACCENT)),
        Line::from("  f             follow on/off       Home/End  top / bottom"),
        Line::from("  PgUp PgDn     scroll"),
    ])
}

fn popup(frame: &mut Frame, title: &str, text: Text<'_>, width: u16, height: u16) {
    let area = centred(frame.area(), width, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(bordered(title)),
        area,
    );
}

/// A box of at most `width` x `height`, centred, never larger than the screen.
fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let [_, middle, _] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .areas(area);
    let [_, centre, _] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(width)) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .areas(middle);
    centre
}

fn bordered(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(MUTED))
        .title(title.to_string())
}
