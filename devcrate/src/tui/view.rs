//! Drawing. Reads the app, writes to a frame, decides nothing.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, HighlightSpacing, List, ListItem, ListState, Padding,
    Paragraph, Row, Table, TableState, Wrap,
};

use crate::status::{ServiceStatus, State};
use crate::tui::app::{App, ExtRow, Modal, PhpPurpose, Tab, Welcome};
use crate::tui::logs;

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const OK: Color = Color::Green;
const WARN: Color = Color::Yellow;
const BAD: Color = Color::Red;
/// The selected row: a soft bar rather than reversed video, so the row's own
/// colours (a red `crashed`, a green port) survive being selected.
const SELECTED: Color = Color::Rgb(38, 50, 66);

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn render(frame: &mut Frame, app: &App) {
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(3), Constraint::Length(2)])
            .areas(frame.area());

    render_header(frame, app, header);
    match (&app.welcome, app.tab) {
        (Some(welcome), _) => render_welcome(frame, app, welcome, body),
        (None, Tab::Services) => render_services(frame, app, body),
        (None, Tab::Sites) => render_sites(frame, app, body),
        (None, Tab::Logs) => render_logs(frame, app, body),
    }
    render_footer(frame, app, footer);

    if let Some(modal) = &app.modal {
        render_modal(frame, app, modal);
    }
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(MUTED));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut tabs = vec![
        Span::styled(" ◆ devcrate ", Style::new().bg(ACCENT).fg(Color::Black).bold()),
        Span::raw("  "),
    ];
    for (i, tab) in Tab::ALL.iter().enumerate() {
        if *tab == app.tab {
            tabs.push(Span::styled(
                format!(" {} {} ", i + 1, tab.title()),
                Style::new().bg(SELECTED).fg(ACCENT).bold(),
            ));
        } else {
            tabs.push(Span::styled(format!(" {}", i + 1), Style::new().fg(ACCENT)));
            tabs.push(Span::styled(format!(" {} ", tab.title()), Style::new().fg(MUTED)));
        }
        tabs.push(Span::raw(" "));
    }
    let tabs = Line::from(tabs);

    let php = app
        .report
        .as_ref()
        .and_then(|report| report.cli_php.clone())
        .unwrap_or_else(|| "not set".into());
    let mut right = summary_spans(app);
    if !right.is_empty() {
        right.push(Span::styled("  │  ", Style::new().fg(MUTED)));
    }
    right.extend([
        Span::styled("CLI PHP ", Style::new().fg(MUTED)),
        Span::styled(php, Style::new().fg(ACCENT).bold()),
        Span::raw(" "),
    ]);
    let mut right = Line::from(right);

    let [left_area, right_area] =
        Layout::horizontal([Constraint::Length(tabs.width() as u16), Constraint::Min(0)])
            .areas(inner);
    // The root is the first thing to go on a narrow screen: it is the one
    // fact here that never changes while the dashboard is open.
    let root = format!("  │  {} ", app.stack.root.display());
    if right.width() + root.chars().count() <= right_area.width as usize {
        right.spans.pop();
        right.spans.push(Span::styled(root, Style::new().fg(MUTED)));
    }
    frame.render_widget(Paragraph::new(tabs), left_area);
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), right_area);
}

/// `● 3/4 up  ✗ 1 crashed` -- the stack at a glance, for the header.
fn summary_spans(app: &App) -> Vec<Span<'static>> {
    let Some(report) = &app.report else {
        return Vec::new();
    };
    let installed: Vec<&ServiceStatus> =
        report.services.iter().filter(|service| service.installed).collect();
    let up = installed.iter().filter(|service| service.state == State::Up).count();
    let crashed = installed.iter().filter(|service| app.crashed(&service.id)).count();

    let dot = if up == 0 {
        MUTED
    } else if up == installed.len() {
        OK
    } else {
        WARN
    };
    let mut spans = vec![
        Span::styled("● ", Style::new().fg(dot)),
        Span::styled(format!("{up}/{} up", installed.len()), Style::new().bold()),
    ];
    if crashed > 0 {
        spans.push(Span::styled(format!("  ✗ {crashed} crashed"), Style::new().fg(BAD).bold()));
    }
    spans
}

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

/// The first-run screen: what `s` will do, then -- once it is running -- each
/// step as it finishes, and why it stopped if it did.
fn render_welcome(frame: &mut Frame, app: &App, welcome: &Welcome, area: Rect) {
    let muted = Style::new().fg(MUTED);
    let started = app.busy.is_some() || !welcome.steps.is_empty() || !welcome.error.is_empty();
    // The root on a line of its own: a stack under a user's temp folder is
    // long enough to wrap mid-sentence otherwise.
    let mut lines = vec![
        Line::from(""),
        Line::from(match started {
            false => "  Nothing is installed in this stack yet:",
            true => "  Setting up this stack:",
        }),
        Line::from(Span::styled(format!("    {}", app.stack.root.display()), Style::new().fg(ACCENT))),
        Line::from(""),
    ];
    if !started {
        lines.extend([
            Line::from(vec![Span::raw("  "), key("s"), Span::raw("    set up a working stack:")]),
            Line::from(Span::styled(
                "         nginx, the newest PHP, MariaDB, Composer and mkcert, each downloaded where missing;",
                muted,
            )),
            Line::from(Span::styled("         mkcert's local CA trusted, then the stack started.", muted)),
            Line::from(Span::styled(
                "         Windows asks once to confirm trusting the CA certificate.",
                Style::new().fg(WARN),
            )),
            Line::from(""),
            Line::from(vec![
                Span::raw("  "),
                key("Esc"),
                Span::raw("  the dashboard as it is -- "),
                key("i"),
                Span::raw(" there installs one thing at a time"),
            ]),
            Line::from(""),
            Line::from(Span::styled("  The same steps run from a terminal as `devcrate setup`.", muted)),
        ]);
    }

    for step in &welcome.steps {
        lines.push(Line::from(vec![
            Span::styled("  ✓ ", Style::new().fg(OK).bold()),
            Span::raw(step.clone()),
        ]));
    }
    // What is happening now, or why it stopped, is pinned to the bottom so a
    // long list of steps can never push it -- or the key to carry on -- off
    // the screen.
    let mut tail = Vec::new();
    if app.busy.is_some() {
        tail.push(Line::from(vec![
            Span::styled(format!("  {} ", spinner()), Style::new().fg(WARN).bold()),
            Span::styled(app.progress.clone().unwrap_or_else(|| "working...".into()), muted),
        ]));
        // mkcert -install waits on a Windows dialog that can open behind this
        // window, and nothing here can tell that apart from a slow step.
        tail.push(Line::from(""));
        tail.push(Line::from(Span::styled(
            "  At the local CA step Windows asks you to confirm -- look behind this window if nothing moves.",
            Style::new().fg(WARN),
        )));
    }
    if let Some((reason, rest)) = welcome.error.split_first() {
        tail.push(Line::from(Span::styled(format!("  ✗ {reason}"), Style::new().fg(BAD))));
        for line in rest {
            tail.push(Line::from(Span::styled(format!("    {line}"), muted)));
        }
        tail.push(Line::from(""));
        tail.push(Line::from(vec![
            Span::raw("  "),
            key("s"),
            Span::raw(" carry on    "),
            key("Esc"),
            Span::raw(" the dashboard"),
        ]));
    }

    let block = focused("Welcome to devcrate");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Rows a wrapped line takes, plus one to spare: word wrapping can need
    // more than the plain division says.
    let width = inner.width.max(1) as usize;
    let tail_rows: usize =
        tail.iter().map(|line| line.width().max(1).div_ceil(width) + usize::from(line.width() > width)).sum();
    let [top, bottom] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(tail_rows.min(inner.height as usize / 2 + 2) as u16)])
            .areas(inner);
    // Steps are not wrapped, so the newest can be kept in view exactly: a
    // long path is cut at the edge rather than spilling onto the next row.
    let paragraph = match started {
        false => Paragraph::new(lines).wrap(Wrap { trim: false }),
        true => {
            let scroll = lines.len().saturating_sub(top.height as usize) as u16;
            Paragraph::new(lines).scroll((scroll, 0))
        }
    };
    frame.render_widget(paragraph, top);
    frame.render_widget(Paragraph::new(tail).wrap(Wrap { trim: false }), bottom);
}

fn render_services(frame: &mut Frame, app: &App, area: Rect) {
    let Some(report) = &app.report else {
        frame.render_widget(
            Paragraph::new(Line::from(format!(" {} Scanning...", spinner()).fg(MUTED)))
                .block(focused("Services")),
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
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Min(20),
    ];
    let header = table_header(["SERVICE", "STATE", "PORTS", "UPTIME", "PIDS", "PATH"]);

    let mut state = TableState::default().with_selected(Some(app.service_row));
    frame.render_stateful_widget(
        Table::new(rows, widths)
            .header(header)
            .block(focused(&format!("Services ({})", report.services.len())))
            .column_spacing(2)
            .row_highlight_style(highlight())
            .highlight_symbol("▌ ")
            .highlight_spacing(HighlightSpacing::Always),
        table_area,
        &mut state,
    );

    if !holders.is_empty() {
        let lines: Vec<Line> =
            holders.into_iter().map(|line| Line::from(format!(" ⚠ {line}").fg(BAD))).collect();
        frame.render_widget(
            Paragraph::new(lines).block(
                bordered("Ports held by something else").border_style(Style::new().fg(BAD)),
            ),
            notice_area,
        );
    }
}

fn service_row<'a>(app: &App, service: &'a ServiceStatus) -> Row<'a> {
    // The icon is a second channel beside the word, never instead of it.
    let (icon, label, style) = if app.crashed(&service.id) {
        ("✗", "crashed", Style::new().fg(BAD).bold())
    } else {
        let style = Style::new().fg(state_color(service.state));
        let icon = match service.state {
            State::Up => "●",
            State::Starting => "◐",
            State::PortBusy => "⚠",
            State::Stopped => "○",
            State::Absent => "·",
        };
        (icon, service.state.as_str(), style)
    };

    let ports = if service.ports.is_empty() {
        Line::from("-".fg(MUTED))
    } else {
        let mut spans = Vec::new();
        for (i, port) in service.ports.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(if port.listening {
                Span::styled(port.port.to_string(), Style::new().fg(OK))
            } else {
                Span::styled(format!("({})", port.port), Style::new().fg(MUTED))
            });
        }
        Line::from(spans)
    };

    let pids = match service.pids.split_first() {
        None => "-".to_string(),
        Some((first, [])) => first.to_string(),
        Some((first, rest)) => format!("{first} +{}", rest.len()),
    };

    let name = if service.installed { Style::new().bold() } else { Style::new().fg(MUTED) };
    Row::new(vec![
        Cell::from(service.name.clone()).style(name),
        Cell::from(format!("{icon} {label}")).style(style),
        Cell::from(ports),
        Cell::from(crate::status::format_uptime(service.uptime_secs)),
        Cell::from(pids).style(Style::new().fg(MUTED)),
        Cell::from(service.path.clone()).style(Style::new().fg(MUTED)),
    ])
}

fn state_color(state: State) -> Color {
    match state {
        State::Up => OK,
        State::Starting => WARN,
        State::PortBusy => BAD,
        State::Stopped | State::Absent => MUTED,
    }
}

// ---------------------------------------------------------------------------
// Sites
// ---------------------------------------------------------------------------

fn render_sites(frame: &mut Frame, app: &App, area: Rect) {
    if app.sites.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from("No vhosts yet.".fg(MUTED)).centered(),
                Line::from(vec![
                    Span::styled("Press ", Style::new().fg(MUTED)),
                    key("n"),
                    Span::styled(" to create one.", Style::new().fg(MUTED)),
                ])
                .centered(),
            ])
            .block(focused("Sites")),
            area,
        );
        return;
    }

    let rows: Vec<Row> = app
        .sites
        .iter()
        .map(|site| {
            let php = match app.php_for_port(site.fastcgi_port) {
                Some(name) => Span::styled(name.to_string(), Style::new().fg(ACCENT)),
                None => Span::styled(
                    site.fastcgi_port
                        .map(|port| format!("port {port}"))
                        .unwrap_or_else(|| "no fastcgi_pass".into()),
                    Style::new().fg(WARN),
                ),
            };
            let root = site.root.clone().unwrap_or_else(|| "no root directive".into());
            let dev = app
                .report
                .as_ref()
                .and_then(|report| report.dev.iter().find(|dev| dev.host == site.host))
                .map(|dev| match dev.running {
                    true => Span::styled(
                        dev.url
                            .as_deref()
                            .and_then(|url| url.trim_end_matches('/').rsplit(':').next())
                            .map(|port| format!("● :{port}"))
                            .unwrap_or_else(|| "● running".into()),
                        Style::new().fg(OK),
                    ),
                    false => Span::styled("✗ exited", Style::new().fg(BAD)),
                })
                .unwrap_or_else(|| Span::styled("-", Style::new().fg(MUTED)));
            let cert = app
                .report
                .as_ref()
                .and_then(|report| report.certs.iter().find(|cert| cert.host == site.host))
                .map(cert_cell)
                .unwrap_or_else(|| Span::styled("-", Style::new().fg(MUTED)));
            Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled(if site.tls { "https://" } else { "http://" }, Style::new().fg(MUTED)),
                    Span::styled(site.host.clone(), Style::new().bold()),
                ])),
                Cell::from(Line::from(php)),
                Cell::from(Line::from(dev)),
                Cell::from(Line::from(cert)),
                Cell::from(root).style(Style::new().fg(MUTED)),
            ])
        })
        .collect();

    let mut state = TableState::default().with_selected(Some(app.site_row));
    frame.render_stateful_widget(
        Table::new(
            rows,
            [
                Constraint::Min(24),
                Constraint::Length(12),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Min(20),
            ],
        )
        .header(table_header(["VHOST", "PHP", "DEV", "CERT", "ROOT"]))
        .block(focused(&format!("Sites ({})", app.sites.len())))
        .column_spacing(2)
        .row_highlight_style(highlight())
        .highlight_symbol("▌ ")
        .highlight_spacing(HighlightSpacing::Always),
        area,
        &mut state,
    );
}

/// A site's certificate in a few characters: how long it has, or what is wrong.
fn cert_cell(cert: &crate::cert::SiteCert) -> Span<'static> {
    use crate::cert::CertState;
    let days = cert.days_left.unwrap_or(0);
    match cert.state {
        CertState::Ok if days >= 365 => Span::styled(format!("✓ {}y", days / 365), Style::new().fg(OK)),
        CertState::Ok => Span::styled(format!("✓ {days}d"), Style::new().fg(OK)),
        CertState::Expiring => Span::styled(format!("! {days}d"), Style::new().fg(WARN)),
        CertState::Expired => Span::styled("✗ expired", Style::new().fg(BAD)),
        CertState::NotCovering => Span::styled("✗ name", Style::new().fg(BAD)),
        CertState::ForeignCa => Span::styled("✗ CA", Style::new().fg(BAD)),
        CertState::Missing => Span::styled("✗ missing", Style::new().fg(BAD)),
        CertState::Unreadable => Span::styled("✗ bad", Style::new().fg(BAD)),
    }
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

fn render_logs(frame: &mut Frame, app: &App, area: Rect) {
    let [picker_area, tail_area] =
        Layout::horizontal([Constraint::Length(36), Constraint::Min(30)]).areas(area);

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
            .block(focused(&format!("Files ({})", app.logs.len())))
            .highlight_style(highlight())
            .highlight_symbol("▌ ")
            .highlight_spacing(HighlightSpacing::Always),
        picker_area,
        &mut state,
    );

    let Some(view) = &app.log else {
        let message = if app.logs.is_empty() {
            "No log files yet. They appear once the services have run."
        } else {
            "Reading..."
        };
        frame.render_widget(
            Paragraph::new(Line::from(format!(" {message}").fg(MUTED))).block(bordered("Log")),
            tail_area,
        );
        return;
    };

    // Show the window ending `log_scroll` lines above the last line.
    let height = tail_area.height.saturating_sub(2) as usize;
    let end = view.lines.len().saturating_sub(app.log_scroll);
    let start = end.saturating_sub(height);
    let mut lines: Vec<Line> = view.lines[start..end].iter().map(|line| log_line(line)).collect();
    if start == 0 && view.truncated {
        lines.insert(0, Line::from("(earlier lines not shown)".fg(MUTED)));
    }
    if lines.is_empty() {
        lines.push(Line::from("(empty)".fg(MUTED)));
    }

    let badge = if app.follow {
        Span::styled(" ● following ", Style::new().fg(OK))
    } else {
        Span::styled(format!(" ‖ paused, {} lines up ", app.log_scroll), Style::new().fg(WARN))
    };
    let title = view.path.display().to_string();
    frame.render_widget(
        Paragraph::new(lines)
            .block(bordered(&title).title_bottom(Line::from(badge).right_aligned())),
        tail_area,
    );
}

/// Colour a log line by the severity it names, as nginx, PHP, and MariaDB spell
/// it. Only a hint: the text itself is shown unchanged.
fn log_line(line: &str) -> Line<'_> {
    let lower = line.to_ascii_lowercase();
    let mentions = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    let colour = if mentions(&["[error]", "[crit]", "[alert]", "[emerg]", "fatal error", "[error "]) {
        Some(BAD)
    } else if mentions(&["[warn]", "[warning]", "warning:"]) {
        Some(WARN)
    } else if mentions(&["[notice]", "[note]", "[info]"]) {
        Some(MUTED)
    } else {
        None
    };
    match colour {
        Some(colour) => Line::from(Span::styled(line, Style::new().fg(colour))),
        None => Line::from(line),
    }
}

// ---------------------------------------------------------------------------
// Footer and modals
// ---------------------------------------------------------------------------

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let [note_area, keys_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    let note = match (&app.busy, &app.note) {
        (Some(busy), _) => Line::from(vec![
            Span::styled(format!(" {} ", spinner()), Style::new().fg(WARN).bold()),
            Span::styled(" working ", Style::new().bg(WARN).fg(Color::Black).bold()),
            Span::raw(format!(" {busy}...")),
            Span::styled(
                app.progress.as_deref().map(|p| format!("  {p}")).unwrap_or_default(),
                Style::new().fg(ACCENT),
            ),
        ]),
        (None, Some(note)) if !note.is_stale() => {
            let (icon, colour) = if note.failed { ("✗", BAD) } else { ("✓", OK) };
            Line::from(Span::styled(format!(" {icon} {}", note.text), Style::new().fg(colour)))
        }
        _ => Line::from(Span::styled(" ● ready", Style::new().fg(MUTED))),
    };
    frame.render_widget(Paragraph::new(note), note_area);

    let keys: &[(&str, &str)] = match app.tab {
        _ if app.welcome.is_some() && app.busy.is_some() => &[("q", "quit")],
        _ if app.welcome.is_some() => &[("s", "set up"), ("Esc", "skip to the dashboard"), ("q", "quit")],
        Tab::Services => &[
            ("s", "start"),
            ("x", "stop"),
            ("t", "restart"),
            ("S X T", "all"),
            ("i", "install"),
            ("U", "uninstall"),
            ("u", "CLI PHP"),
            ("e", "PHP ext"),
            ("?", "help"),
            ("q", "quit"),
        ],
        Tab::Sites => &[
            ("n", "new"),
            ("N", "new project"),
            ("p", "PHP"),
            ("v", "dev"),
            ("o", "browser"),
            ("e", "folder"),
            ("c", "terminal"),
            ("d", "delete"),
            ("?", "help"),
            ("q", "quit"),
        ],
        Tab::Logs => &[
            ("f", "follow"),
            ("PgUp PgDn", "scroll"),
            ("Home End", "top / bottom"),
            ("?", "help"),
            ("q", "quit"),
        ],
    };
    let mut spans = vec![Span::raw(" ")];
    for (i, (name, what)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(key(name));
        spans.push(Span::styled(format!(" {what}"), Style::new().fg(MUTED)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), keys_area);
}

fn render_modal(frame: &mut Frame, app: &App, modal: &Modal) {
    match modal {
        Modal::Help => {
            // As tall as its text plus the border, so a new key cannot push
            // the last row out of the box.
            let text = help_text();
            let height = text.height() as u16 + 2;
            popup(frame, "Keys", text, 72, height)
        }
        Modal::Output { title, lines, failed } => {
            let colour = if *failed { BAD } else { Color::Reset };
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
            text.push_line(Line::from(vec![
                key("y"),
                Span::styled(" confirm     any other key cancels", Style::new().fg(MUTED)),
            ]));
            let area = centred(frame.area(), 64, 9);
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(text).wrap(Wrap { trim: false }).block(
                    Block::bordered()
                        .border_type(BorderType::Thick)
                        .border_style(Style::new().fg(WARN))
                        .title(Line::from(" ⚠ Confirm ".fg(WARN).bold()).centered())
                        .padding(Padding::horizontal(1)),
                ),
                area,
            );
        }
        Modal::NewSite(form) => {
            let versions = app.php_versions();
            let php_label = if form.php_index == 0 {
                "Auto-detect (composer.json / default)".to_string()
            } else if let Some((_, name)) = versions.get(form.php_index - 1) {
                name.clone()
            } else {
                "Auto-detect".to_string()
            };

            let f_style = |idx| {
                if form.active_field == idx {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(Color::Reset)
                }
            };
            let label = |idx, text: &'static str| {
                let marker = if form.active_field == idx { "› " } else { "  " };
                Span::styled(format!("{marker}{text:<17}"), f_style(idx))
            };
            let cursor = |idx| {
                if form.active_field == idx {
                    Span::styled("▏", Style::new().fg(ACCENT).add_modifier(Modifier::SLOW_BLINK))
                } else {
                    Span::raw("")
                }
            };
            let button = |idx, text: &'static str| {
                if form.active_field == idx {
                    Span::styled(text, Style::new().bg(ACCENT).fg(Color::Black).bold())
                } else {
                    Span::styled(text, Style::new().fg(ACCENT))
                }
            };
            let check = |on: bool| if on { "[✓] " } else { "[ ] " };

            let text = Text::from(vec![
                Line::from(vec![label(0, "Hostname"), Span::styled(&form.host, f_style(0)), cursor(0)]),
                Line::from(vec![
                    label(1, "Project Path"),
                    if form.path.is_empty() {
                        Span::styled("projects/<host>  (default)", Style::new().fg(MUTED))
                    } else {
                        Span::styled(&form.path, f_style(1))
                    },
                    cursor(1),
                ]),
                Line::from(vec![Span::raw(" ".repeat(19)), button(2, " Browse Folder... ")]),
                Line::from(vec![
                    label(3, "PHP Version"),
                    Span::styled("‹ ", Style::new().fg(MUTED)),
                    Span::styled(&php_label, f_style(3)),
                    Span::styled(" ›", Style::new().fg(MUTED)),
                ]),
                Line::from(vec![
                    label(4, "Hosts File"),
                    Span::styled(
                        format!(
                            "{}{}",
                            check(form.update_hosts),
                            if form.update_hosts { "Auto-add to Windows hosts" } else { "Skip hosts update" }
                        ),
                        f_style(4),
                    ),
                ]),
                Line::from(vec![
                    label(5, "TLS Certificate"),
                    Span::styled(
                        format!(
                            "{}{}",
                            check(form.issue_tls),
                            if form.issue_tls { "Auto-issue wildcard SSL (mkcert)" } else { "Skip TLS certificate" }
                        ),
                        f_style(5),
                    ),
                ]),
                Line::from(""),
                Line::from(vec![Span::raw(" ".repeat(19)), button(6, "  Create Site  ")]),
                Line::from(""),
                Line::from(vec![
                    key("Tab j k"),
                    Span::styled(" fields  ·  ", Style::new().fg(MUTED)),
                    key("Enter b"),
                    Span::styled(" browse  ·  ", Style::new().fg(MUTED)),
                    key("Esc"),
                    Span::styled(" cancel", Style::new().fg(MUTED)),
                ]),
            ]);
            popup(frame, "Create New Site (vhost)", text, 72, 12);
        }
        Modal::NewProject(form) => {
            let versions = app.php_versions();
            let php_label = match form.php_index.checked_sub(1).and_then(|i| versions.get(i)) {
                Some((_, name)) => name.clone(),
                None => "php\\current".to_string(),
            };
            let style = |idx| match form.active_field == idx {
                true => Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                false => Style::new().fg(Color::Reset),
            };
            let label = |idx, text: &'static str| {
                let marker = if form.active_field == idx { "› " } else { "  " };
                Span::styled(format!("{marker}{text:<12}"), style(idx))
            };
            let choice = |idx, text: String| {
                vec![
                    Span::styled("‹ ", Style::new().fg(MUTED)),
                    Span::styled(text, style(idx)),
                    Span::styled(" ›", Style::new().fg(MUTED)),
                ]
            };
            let host = crate::create::names(form.name.trim()).map(|(host, _)| host).unwrap_or_default();
            let mut lines = vec![
                Line::from([vec![label(0, "Kind")], choice(0, form.kind().label().to_string())].concat()),
                Line::from(vec![
                    label(1, "Name"),
                    Span::styled(form.name.clone(), style(1)),
                    if form.active_field == 1 {
                        Span::styled("▏", Style::new().fg(ACCENT).add_modifier(Modifier::SLOW_BLINK))
                    } else {
                        Span::raw("")
                    },
                    Span::styled(
                        if host.is_empty() { String::new() } else { format!("   https://{host}") },
                        Style::new().fg(MUTED),
                    ),
                ]),
                Line::from([vec![label(2, "PHP")], choice(2, php_label)].concat()),
                Line::from([vec![label(3, "Database")], choice(3, form.database().label().to_string())].concat()),
                Line::from(""),
                Line::from(vec![
                    Span::raw(" ".repeat(14)),
                    if form.active_field == 4 {
                        Span::styled("  Create Project  ", Style::new().bg(ACCENT).fg(Color::Black).bold())
                    } else {
                        Span::styled("  Create Project  ", Style::new().fg(ACCENT))
                    },
                ]),
            ];
            lines.push(match &form.error {
                Some(error) => Line::from(Span::styled(error.clone(), Style::new().fg(BAD))),
                None => Line::from(Span::styled(
                    "Into projects\\<name>, never over a folder with content. Composer's output goes to logs\\.",
                    Style::new().fg(MUTED),
                )),
            });
            lines.push(Line::from(vec![
                key("Tab ↑↓"),
                Span::styled(" fields  ·  ", Style::new().fg(MUTED)),
                key("← →"),
                Span::styled(" choose  ·  ", Style::new().fg(MUTED)),
                key("Enter"),
                Span::styled(" create  ·  ", Style::new().fg(MUTED)),
                key("Esc"),
                Span::styled(" cancel", Style::new().fg(MUTED)),
            ]));
            popup(frame, "New project (devcrate new)", Text::from(lines), 80, 12);
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
                .map(|(id, name)| {
                    ListItem::new(Line::from(vec![
                        Span::raw(name.clone()),
                        Span::styled(format!("   {id}"), Style::new().fg(MUTED)),
                    ]))
                })
                .collect();
            picker(frame, &title, items, *index);
        }
        Modal::InstallPicker { index } => {
            let items: Vec<ListItem> = app
                .install_options
                .iter()
                .map(|offering| ListItem::new(offering.label.clone()))
                .collect();
            let title = if app.offerings_loaded {
                "Install Service / Runtime".to_string()
            } else {
                format!("Install Service / Runtime  {} reading catalogues", spinner())
            };
            picker(frame, &title, items, *index);
        }
        Modal::UninstallPicker { index, items } => {
            let items: Vec<ListItem> = items
                .iter()
                .map(|item| ListItem::new(item.label.clone()))
                .collect();
            picker(frame, "Uninstall", items, *index);
        }
        Modal::ExtPicker { php_name, index, rows, .. } => {
            let items: Vec<ListItem> = rows.iter().map(ext_row).collect();
            let title = format!("{php_name} extensions -- Enter switches, Esc closes");
            picker(frame, &title, items, *index);
        }
    }
}

fn ext_row(row: &ExtRow) -> ListItem<'static> {
    use crate::php_ext::Source;
    match row {
        ExtRow::Install(name) => ListItem::new(Line::from(vec![
            Span::styled("get ", Style::new().fg(ACCENT)),
            Span::raw(format!("{name:<14}")),
            Span::styled("not installed -- Enter downloads a matching build", Style::new().fg(MUTED)),
        ])),
        ExtRow::Ext(ext) => {
            let (mark, colour) = match ext.enabled {
                Some(_) => ("on  ", OK),
                None => ("off ", MUTED),
            };
            let note = match &ext.enabled {
                Some(Source::Ini(line)) => format!("php.ini line {line} -- yours, not switched from here"),
                Some(Source::Block) => "devcrate".to_string(),
                None => String::new(),
            };
            ListItem::new(Line::from(vec![
                Span::styled(mark, Style::new().fg(colour)),
                Span::raw(format!("{:<14}", ext.name)),
                Span::styled(note, Style::new().fg(MUTED)),
            ]))
        }
    }
}

fn picker(frame: &mut Frame, title: &str, items: Vec<ListItem>, index: usize) {
    if items.is_empty() {
        return popup(frame, title, Text::from(" Nothing is installed.".fg(MUTED)), 48, 3);
    }
    let height = (items.len() + 2).max(3) as u16;
    // Wide enough for the longest entry and the title, plus the marker and
    // the frame; never narrower than the 48 the short lists use.
    let widest = items.iter().map(ListItem::width).max().unwrap_or(0);
    let width = (widest + 6).max(title.chars().count() + 6).max(48) as u16;
    let area = centred(frame.area(), width, height);
    frame.render_widget(Clear, area);
    let mut state = ListState::default().with_selected(Some(index));
    frame.render_stateful_widget(
        List::new(items)
            .block(modal_block(title))
            .highlight_style(highlight())
            .highlight_symbol("› ")
            .highlight_spacing(HighlightSpacing::Always),
        area,
        &mut state,
    );
}

fn help_text() -> Text<'static> {
    let section = |name: &'static str| Line::from(name.fg(ACCENT).bold());
    let row = |keys: &'static str, what: &'static str| {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{keys:<14}"), Style::new().fg(ACCENT)),
            Span::raw(what),
        ])
    };
    Text::from(vec![
        section("Anywhere"),
        row("1 2 3 / Tab", "switch pane"),
        row("up down j k", "move"),
        row("r  /  q", "rescan now  /  quit"),
        Line::from(""),
        section("Services"),
        row("s / x / t", "start / stop / restart the selected service"),
        row("S / X / T", "...the whole stack, in dependency order"),
        row("i", "install service or runtime (Node, Bun, PHP, etc)"),
        row("U", "uninstall one (never forces, never deletes data)"),
        row("u", "repoint php\\current (the CLI version)"),
        row("e", "a PHP version's extensions, Xdebug too"),
        row("A", "autostart on / off (devcrate start at logon)"),
        Line::from(""),
        section("Sites"),
        row("n", "new vhost"),
        row("N", "new Laravel / WordPress project"),
        row("v", "the project's dev server (npm run dev) on / off"),
        row("R", "issue the site's certificate again (mkcert)"),
        row("p", "change PHP version"),
        row("o / e / c", "open in the browser / Explorer / a terminal"),
        row("d", "delete the conf"),
        Line::from(""),
        section("Logs"),
        row("f", "follow on/off"),
        row("PgUp PgDn", "scroll"),
        row("Home / End", "top / bottom"),
    ])
}

fn popup(frame: &mut Frame, title: &str, text: Text<'_>, width: u16, height: u16) {
    let area = centred(frame.area(), width, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(modal_block(title).padding(Padding::horizontal(1))),
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

/// A key name, as the footer, help, and forms show it.
fn key(name: &str) -> Span<'static> {
    Span::styled(name.to_string(), Style::new().fg(ACCENT).bold())
}

fn bordered(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(MUTED))
        .title(Line::from(format!(" {title} ").bold()))
}

/// The pane that has the keyboard: the same box with an accent frame.
fn focused(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::from(format!(" {title} ").fg(ACCENT).bold()))
}

/// A popup's frame: heavier than a pane, so it reads as sitting on top.
fn modal_block(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::from(format!(" {title} ").fg(ACCENT).bold()).centered())
}

fn table_header<const N: usize>(names: [&'static str; N]) -> Row<'static> {
    Row::new(names).style(Style::new().fg(MUTED).add_modifier(Modifier::BOLD)).bottom_margin(1)
}

fn highlight() -> Style {
    Style::new().bg(SELECTED).add_modifier(Modifier::BOLD)
}

/// One frame of the busy spinner, picked by the wall clock so the view stays a
/// function of the app and nothing else needs to count frames.
fn spinner() -> &'static str {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    SPINNER[(millis / 100) as usize % SPINNER.len()]
}
