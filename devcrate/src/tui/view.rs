//! Drawing. Reads the app, writes to a frame, decides nothing.
//!
//! The look follows ratatui's own example apps (`demo2`, `table`, `popup`):
//!
//! - **Panels, not boxes.** Each pane is a block of its own background, set
//!   one column in from the edge, with its name on its first row. Lines are
//!   drawn only where they carry meaning: the coloured edge of an overview
//!   card, a popup's frame.
//! - **One-row bars.** The brand and the stack at a glance on top, the tabs
//!   under it -- the active one in the panel's colour, so it joins the pane it
//!   names -- and at the bottom a status line over a bar of keys, each key in a
//!   small block of its own.
//! - **Striped tables** with a tinted selection bar and an accent edge, and a
//!   thumb-only scrollbar that appears only when there is more than fits.
//!
//! Colours come from [`theme`], which also handles `NO_COLOR`. Every glyph is
//! box drawing, a block element, or a plain geometric shape, so no icon font
//! is needed: JetBrains Mono, Cascadia, and Consolas all carry them.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, HighlightSpacing, List, ListItem, ListState, Padding,
    Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap,
};

use crate::cert::CertState;
use crate::site::Site;
use crate::status::{ServiceStatus, State, StatusReport};
use crate::tui::app::{App, ExtRow, Modal, PhpPurpose, Tab, Welcome};
use crate::tui::logs;
use crate::tui::theme::theme;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The overview cards cost five rows; below this height the table needs them.
const OVERVIEW_MIN_HEIGHT: u16 = 28;
/// The detail panel beside a table costs [`DETAIL_WIDTH`] columns; below this
/// width the table's own PATH / ROOT column needs them.
const DETAIL_MIN_WIDTH: u16 = 118;
const DETAIL_WIDTH: u16 = 44;
/// Panels sit this far in from the left and right edges of the screen.
const GUTTER: Margin = Margin { horizontal: 1, vertical: 0 };

pub fn render(frame: &mut Frame, app: &App) {
    let t = theme();
    let area = frame.area();
    frame.render_widget(Block::new().style(Style::new().bg(t.bg).fg(t.text)), area);

    let overview =
        app.welcome.is_none() && app.report.is_some() && area.height >= OVERVIEW_MIN_HEIGHT;
    let [brand, cards, tabs, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if overview { 5 } else { 0 }),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .areas(area);

    render_brand(frame, app, brand);
    if overview && let Some(report) = &app.report {
        let [_, cards, _] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(3), Constraint::Length(1)])
                .areas(cards);
        render_overview(frame, app, report, cards.inner(GUTTER));
    }
    render_tabs(frame, app, tabs.inner(GUTTER));
    let body = body.inner(GUTTER);
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

// ---------------------------------------------------------------------------
// The top two rows
// ---------------------------------------------------------------------------

/// Who this is, then -- on the right -- the stack at a glance and its root.
fn render_brand(frame: &mut Frame, app: &App, area: Rect) {
    let t = theme();
    let brand = Line::from(vec![
        Span::styled(" ◆ devcrate ", t.block(t.on_accent, t.accent).bold()),
        Span::styled(concat!(" v", env!("CARGO_PKG_VERSION")), Style::new().fg(t.faint)),
    ]);
    let [left, right] =
        Layout::horizontal([Constraint::Length(brand.width() as u16), Constraint::Min(0)])
            .areas(area);
    frame.render_widget(Paragraph::new(brand), left);

    let php = app
        .report
        .as_ref()
        .and_then(|report| report.cli_php.clone())
        .unwrap_or_else(|| "not set".into());
    let mut glance = summary_spans(app);
    if !glance.is_empty() {
        glance.push(Span::styled("  │  ", Style::new().fg(t.border)));
    }
    glance.extend([
        Span::styled("CLI PHP ", Style::new().fg(t.muted)),
        Span::styled(php, Style::new().fg(t.info).bold()),
    ]);
    // The root is the first thing to go on a narrow screen: it is the one
    // fact here that never changes while the dashboard is open.
    let root = format!("{}", app.stack.root.display());
    let room = right.width as usize;
    let glance_width: usize = glance.iter().map(Span::width).sum();
    if glance_width + root.chars().count() + 8 <= room {
        glance.push(Span::styled("  │  ", Style::new().fg(t.border)));
        glance.push(Span::styled(root, Style::new().fg(t.faint)));
    }
    glance.push(Span::raw(" "));
    frame.render_widget(Paragraph::new(Line::from(glance)).alignment(Alignment::Right), right);
}

/// The panes, by number and name. The active tab is drawn in the panel's own
/// colour, so it reads as the top of the pane below it. The first-run screen
/// takes no tab keys, so there no tab is active.
fn render_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let t = theme();
    let mut spans = Vec::new();
    for (i, tab) in Tab::ALL.iter().enumerate() {
        if *tab == app.tab && app.welcome.is_none() {
            let on = t.block(t.text, t.panel);
            spans.push(Span::styled(format!(" {}", i + 1), on.fg(t.accent).bold()));
            spans.push(Span::styled(format!(" {} ", tab.title()), on.bold()));
        } else {
            spans.push(Span::styled(format!(" {}", i + 1), Style::new().fg(t.faint)));
            spans.push(Span::styled(format!(" {} ", tab.title()), Style::new().fg(t.muted)));
        }
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// (installed, up, crashed) -- the three numbers the header and the first
/// card are both made of.
fn tally(app: &App, report: &StatusReport) -> (usize, usize, usize) {
    let installed: Vec<&ServiceStatus> =
        report.services.iter().filter(|service| service.installed).collect();
    let up = installed.iter().filter(|service| service.state == State::Up).count();
    let crashed = installed.iter().filter(|service| app.crashed(&service.id)).count();
    (installed.len(), up, crashed)
}

/// Green when everything installed is up, amber when some is, grey when none.
fn tally_colour(installed: usize, up: usize) -> Color {
    let t = theme();
    if up == 0 {
        t.muted
    } else if up == installed {
        t.ok
    } else {
        t.warn
    }
}

/// `● 3/4 up  ✗ 1 crashed` -- the stack at a glance, for the header.
fn summary_spans(app: &App) -> Vec<Span<'static>> {
    let t = theme();
    let Some(report) = &app.report else {
        return Vec::new();
    };
    let (installed, up, crashed) = tally(app, report);
    let mut spans = vec![
        Span::styled("● ", Style::new().fg(tally_colour(installed, up))),
        Span::styled(format!("{up}/{installed} up"), Style::new().fg(t.text).bold()),
    ];
    if crashed > 0 {
        spans.push(Span::styled(format!("  ✗ {crashed} crashed"), Style::new().fg(t.bad).bold()));
    }
    spans
}

// ---------------------------------------------------------------------------
// Overview cards
// ---------------------------------------------------------------------------

/// Four cards over every pane: what is running, what is served, whether its
/// certificates are good, and what the command line resolves to. Only on a
/// screen tall enough to spare the rows.
fn render_overview(frame: &mut Frame, app: &App, report: &StatusReport, area: Rect) {
    let t = theme();
    let [stack, sites, certs, cli] =
        Layout::horizontal([Constraint::Ratio(1, 4); 4]).spacing(1).areas(area);
    let muted = Style::new().fg(t.muted);

    let (installed, up, crashed) = tally(app, report);
    let colour = if crashed > 0 { t.bad } else { tally_colour(installed, up) };
    let mut value = vec![Span::styled(format!("{up}/{installed} up"), Style::new().fg(t.text).bold())];
    if crashed > 0 {
        value.push(Span::styled(format!("  ✗ {crashed} crashed"), Style::new().fg(t.bad).bold()));
    }
    let bar = stack.width.saturating_sub(4).min(24) as usize;
    card(frame, stack, "STACK", colour, Line::from(value), Line::from(gauge(up, installed, bar, colour)));

    let proxied = app.sites.iter().filter(|site| site.proxy_port.is_some()).count();
    let dev = report.dev.iter().filter(|dev| dev.running).count();
    let mut served = vec![Span::styled(
        format!("{} PHP · {proxied} proxied", app.sites.len() - proxied),
        muted,
    )];
    if dev > 0 {
        served.push(Span::styled(format!(" · {dev} dev"), Style::new().fg(t.ok)));
    }
    card(
        frame,
        sites,
        "SITES",
        if app.sites.is_empty() { t.border } else { t.info },
        Line::from(Span::styled(
            match app.sites.len() {
                1 => "1 vhost".to_string(),
                n => format!("{n} vhosts"),
            },
            Style::new().fg(t.text).bold(),
        )),
        Line::from(served),
    );

    let needing = report.certs.iter().filter(|cert| cert.needs_renewal()).count();
    let soonest = report.certs.iter().filter_map(|cert| cert.days_left).min();
    let (edge, value, sub) = match (report.certs.len(), needing) {
        (0, _) => (t.border, Span::styled("none", muted), Span::styled("no HTTPS vhosts", muted)),
        (n, 0) => (
            t.ok,
            Span::styled(format!("✓ all {n} good"), Style::new().fg(t.ok).bold()),
            Span::styled(
                soonest.map(|days| format!("first to expire: {days}d")).unwrap_or_default(),
                muted,
            ),
        ),
        (_, n) => (
            t.warn,
            Span::styled(format!("! {n} to renew"), Style::new().fg(t.warn).bold()),
            Span::styled("R on its row in Sites", muted),
        ),
    };
    card(frame, certs, "CERTIFICATES", edge, Line::from(value), Line::from(sub));

    let (edge, php) = match &report.cli_php {
        Some(php) => (t.accent, Span::styled(php.clone(), Style::new().fg(t.info).bold())),
        None => (t.warn, Span::styled("no PHP on PATH", Style::new().fg(t.warn))),
    };
    card(
        frame,
        cli,
        "COMMAND LINE",
        edge,
        Line::from(php),
        Line::from(Span::styled(
            report.cli_python.clone().unwrap_or_else(|| "no Python".into()),
            muted,
        )),
    );
}

/// A card: a panel with a heavy left edge in the colour of what it reports,
/// its name small above the value.
fn card(frame: &mut Frame, area: Rect, title: &str, edge: Color, value: Line, sub: Line) {
    let t = theme();
    let block = Block::new()
        .borders(Borders::LEFT)
        .border_type(BorderType::Thick)
        .border_style(Style::new().fg(edge))
        .style(Style::new().bg(t.panel).fg(t.text))
        .padding(Padding::horizontal(1));
    let heading = Line::from(Span::styled(title.to_string(), Style::new().fg(t.muted).bold()));
    frame.render_widget(Paragraph::new(vec![heading, value, sub]).block(block), area);
}

/// A bar `width` cells long, `filled` of `total` in `colour`. One glyph in two
/// colours rather than two glyphs, so it is a single unbroken line in any font.
fn gauge(filled: usize, total: usize, width: usize, colour: Color) -> Vec<Span<'static>> {
    let on = if total == 0 { 0 } else { (filled * width).div_ceil(total).min(width) };
    // Without colour the two halves would look the same: draw the empty part
    // in a lighter line instead.
    let off = if theme().mono { "─" } else { "━" };
    vec![
        Span::styled("━".repeat(on), Style::new().fg(colour)),
        Span::styled(off.repeat(width - on), Style::new().fg(theme().border)),
    ]
}

// ---------------------------------------------------------------------------
// The first-run screen
// ---------------------------------------------------------------------------

/// What `s` will do, then -- once it is running -- each step as it finishes,
/// and why it stopped if it did.
fn render_welcome(frame: &mut Frame, app: &App, welcome: &Welcome, area: Rect) {
    let t = theme();
    let muted = Style::new().fg(t.muted);
    let started = app.busy.is_some() || !welcome.steps.is_empty() || !welcome.error.is_empty();
    // The root on a line of its own: a stack under a user's temp folder is
    // long enough to wrap mid-sentence otherwise.
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            match started {
                false => "  Nothing is installed in this stack yet:",
                true => "  Setting up this stack:",
            },
            Style::new().fg(t.text).bold(),
        )),
        Line::from(Span::styled(format!("    {}", app.stack.root.display()), Style::new().fg(t.info))),
        Line::from(""),
    ];
    if !started {
        lines.extend([
            Line::from(vec![Span::raw("  "), key("s"), Span::raw("   set up a working stack:")]),
            Line::from(Span::styled(
                "         nginx, the newest PHP, MariaDB, Composer and mkcert, each downloaded where missing;",
                muted,
            )),
            Line::from(Span::styled("         mkcert's local CA trusted, then the stack started.", muted)),
            Line::from(Span::styled(
                "         Windows asks once to confirm trusting the CA certificate.",
                Style::new().fg(t.warn),
            )),
            Line::from(""),
            Line::from(vec![
                Span::raw("  "),
                key("Esc"),
                Span::raw(" the dashboard as it is -- "),
                key("i"),
                Span::raw(" there installs one thing at a time"),
            ]),
            Line::from(""),
            Line::from(Span::styled("  The same steps run from a terminal as `devcrate setup`.", muted)),
        ]);
    }

    for step in &welcome.steps {
        lines.push(Line::from(vec![
            Span::styled("  ✓ ", Style::new().fg(t.ok).bold()),
            Span::raw(step.clone()),
        ]));
    }
    // What is happening now, or why it stopped, is pinned to the bottom so a
    // long list of steps can never push it -- or the key to carry on -- off
    // the screen.
    let mut tail = Vec::new();
    if app.busy.is_some() {
        tail.push(Line::from(vec![
            Span::styled(format!("  {} ", spinner()), Style::new().fg(t.accent).bold()),
            Span::styled(app.progress.clone().unwrap_or_else(|| "working...".into()), muted),
        ]));
        // mkcert -install waits on a Windows dialog that can open behind this
        // window, and nothing here can tell that apart from a slow step.
        tail.push(Line::from(""));
        tail.push(Line::from(Span::styled(
            "  At the local CA step Windows asks you to confirm -- look behind this window if nothing moves.",
            Style::new().fg(t.warn),
        )));
    }
    if let Some((reason, rest)) = welcome.error.split_first() {
        tail.push(Line::from(Span::styled(format!("  ✗ {reason}"), Style::new().fg(t.bad))));
        for line in rest {
            tail.push(Line::from(Span::styled(format!("    {line}"), muted)));
        }
        tail.push(Line::from(""));
        tail.push(Line::from(vec![
            Span::raw("  "),
            key("s"),
            Span::raw("carry on    "),
            key("Esc"),
            Span::raw("the dashboard"),
        ]));
    }

    let block = panel("Welcome to devcrate");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Rows a wrapped line takes, plus one to spare: word wrapping can need
    // more than the plain division says.
    let width = inner.width.max(1) as usize;
    let tail_rows: usize = tail.iter().map(|line| wrapped_rows(line.width(), width)).sum();
    let [top, bottom] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(tail_rows.min(inner.height as usize / 2 + 2) as u16),
    ])
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

/// Rows a line `len` cells long takes in `width`, with one to spare when it
/// wraps at all: word wrapping can need more than the plain division says.
fn wrapped_rows(len: usize, width: usize) -> usize {
    len.max(1).div_ceil(width.max(1)) + usize::from(len > width)
}

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

/// The table, and beside it -- where the screen is wide enough to spare the
/// columns -- a panel about the selected row.
fn with_detail(area: Rect) -> (Rect, Option<Rect>) {
    if area.width < DETAIL_MIN_WIDTH {
        return (area, None);
    }
    let [table, detail] = Layout::horizontal([Constraint::Min(60), Constraint::Length(DETAIL_WIDTH)])
        .spacing(1)
        .areas(area);
    (table, Some(detail))
}

fn render_services(frame: &mut Frame, app: &App, area: Rect) {
    let t = theme();
    let Some(report) = &app.report else {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {} ", spinner()), Style::new().fg(t.accent)),
                Span::styled("Scanning the stack...", Style::new().fg(t.muted)),
            ]))
            .block(panel("Services")),
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

    let [main, notice_area] = if holders.is_empty() {
        [area, Rect::ZERO]
    } else {
        Layout::vertical([Constraint::Min(3), Constraint::Length(holders.len() as u16 + 1)])
            .spacing(1)
            .areas(area)
    };
    let (table_area, detail_area) = with_detail(main);

    let rows: Vec<Row> = report
        .services
        .iter()
        .enumerate()
        .map(|(i, service)| service_row(app, service).style(stripe(i)))
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Min(20),
    ];
    table(
        frame,
        table_area,
        Table::new(rows, widths)
            .header(table_header(["SERVICE", "STATE", "PORTS", "UPTIME", "PIDS", "PATH"]))
            .block(panel_counted("Services", report.services.len())),
        report.services.len(),
        app.service_row,
    );

    if let Some(detail_area) = detail_area {
        match report.services.get(app.service_row) {
            Some(service) => service_detail(frame, app, service, detail_area),
            None => frame.render_widget(panel("Selected"), detail_area),
        }
    }

    if !holders.is_empty() {
        let lines: Vec<Line> =
            holders.into_iter().map(|line| Line::from(format!("⚠ {line}").fg(t.bad))).collect();
        let block = Block::new()
            .borders(Borders::LEFT)
            .border_type(BorderType::Thick)
            .border_style(Style::new().fg(t.bad))
            .style(Style::new().bg(t.panel))
            .padding(Padding::horizontal(1))
            .title(Line::from(Span::styled("Ports held by something else", Style::new().fg(t.bad).bold())));
        frame.render_widget(Paragraph::new(lines).block(block), notice_area);
    }
}

/// A service's state as an icon, a word, and a colour. The icon is a second
/// channel beside the word, never instead of it.
fn state_badge(app: &App, service: &ServiceStatus) -> (&'static str, &'static str, Style) {
    if app.crashed(&service.id) {
        return ("✗", "crashed", Style::new().fg(theme().bad).bold());
    }
    let icon = match service.state {
        State::Up => "●",
        State::Starting => "◐",
        State::PortBusy => "⚠",
        State::Stopped => "○",
        State::Absent => "·",
    };
    (icon, service.state.as_str(), Style::new().fg(state_color(service.state)))
}

fn pids_text(pids: &[u32]) -> String {
    match pids.split_first() {
        None => "-".to_string(),
        Some((first, [])) => first.to_string(),
        Some((first, rest)) => format!("{first} +{}", rest.len()),
    }
}

fn service_row<'a>(app: &App, service: &'a ServiceStatus) -> Row<'a> {
    let t = theme();
    let (icon, label, style) = state_badge(app, service);

    let ports = if service.ports.is_empty() {
        Line::from("-".fg(t.faint))
    } else {
        let mut spans = Vec::new();
        for (i, port) in service.ports.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(if port.listening {
                Span::styled(port.port.to_string(), Style::new().fg(t.ok))
            } else {
                Span::styled(format!("({})", port.port), Style::new().fg(t.muted))
            });
        }
        Line::from(spans)
    };

    let name = if service.installed { Style::new().fg(t.text).bold() } else { Style::new().fg(t.muted) };
    Row::new(vec![
        Cell::from(service.name.clone()).style(name),
        Cell::from(format!("{icon} {label}")).style(style),
        Cell::from(ports),
        Cell::from(crate::status::format_uptime(service.uptime_secs)),
        Cell::from(pids_text(&service.pids)).style(Style::new().fg(t.muted)),
        Cell::from(service.path.clone()).style(Style::new().fg(t.muted)),
    ])
}

/// One labelled line of a detail panel.
fn field(label: &str, value: impl Into<Span<'static>>) -> Line<'static> {
    Line::from(vec![Span::styled(format!("{label:<8}"), Style::new().fg(theme().muted)), value.into()])
}

/// The sentence at the foot of a detail panel: what to do about the row, in
/// the keys that do it.
fn advice(text: &str) -> Line<'static> {
    let t = theme();
    Line::from(vec![
        Span::styled("› ", Style::new().fg(t.accent)),
        Span::styled(text.to_string(), Style::new().fg(t.muted)),
    ])
}

fn service_detail(frame: &mut Frame, app: &App, service: &ServiceStatus, area: Rect) {
    let t = theme();
    let muted = Style::new().fg(t.muted);
    let (icon, label, style) = state_badge(app, service);
    let mut lines = vec![
        Line::from(Span::styled(service.name.clone(), Style::new().fg(t.text).bold())),
        Line::from(vec![
            Span::styled(format!("{icon} {label}"), style),
            Span::styled(
                match service.uptime_secs {
                    Some(_) => format!("  for {}", crate::status::format_uptime(service.uptime_secs)),
                    None => String::new(),
                },
                muted,
            ),
        ]),
        Line::from(""),
        field("id", Span::styled(service.id.clone(), Style::new().fg(t.info))),
    ];
    for port in &service.ports {
        let (text, colour) = match (&port.holder, port.listening) {
            (Some(who), _) => (format!("{} held by {who}", port.port), t.bad),
            (None, true) => (format!("{} answering", port.port), t.ok),
            (None, false) => (format!("{} closed", port.port), t.muted),
        };
        lines.push(field("port", Span::styled(text, Style::new().fg(colour))));
    }
    lines.push(field("pids", Span::raw(pids_text(&service.pids))));
    lines.push(field("path", Span::styled(service.path.clone(), muted)));
    lines.push(Line::from(""));
    let hint = if !service.installed {
        "Not installed. i picks something to install."
    } else if app.crashed(&service.id) {
        "It went down on its own. s starts it again; 3 has its log."
    } else {
        match service.state {
            State::Up => "x stops it, t restarts it.",
            State::Starting => "Running, but no port answers yet.",
            State::PortBusy => "Something else holds its port, so it cannot start.",
            State::Stopped | State::Absent => "s starts it.",
        }
    };
    lines.push(advice(hint));
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel("Selected").padding(Padding::horizontal(1))),
        area,
    );
}

fn state_color(state: State) -> Color {
    let t = theme();
    match state {
        State::Up => t.ok,
        State::Starting => t.warn,
        State::PortBusy => t.bad,
        State::Stopped | State::Absent => t.muted,
    }
}

// ---------------------------------------------------------------------------
// Sites
// ---------------------------------------------------------------------------

/// What serves a vhost: the PHP version by name, where a proxy forwards, or
/// what is wrong with its conf.
fn serves_span(app: &App, site: &Site) -> Span<'static> {
    let t = theme();
    match (site.proxy_port, app.php_for_port(site.fastcgi_port)) {
        (Some(port), _) => Span::styled(format!("→ :{port}"), Style::new().fg(t.accent)),
        (None, Some(name)) => Span::styled(name.to_string(), Style::new().fg(t.info)),
        (None, None) => Span::styled(
            site.fastcgi_port
                .map(|port| format!("port {port}"))
                .unwrap_or_else(|| "no fastcgi_pass".into()),
            Style::new().fg(t.warn),
        ),
    }
}

fn render_sites(frame: &mut Frame, app: &App, area: Rect) {
    let t = theme();
    if app.sites.is_empty() {
        let muted = Style::new().fg(t.muted);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled("Nothing is served yet.", Style::new().fg(t.text).bold())).centered(),
                Line::from(""),
                Line::from(vec![key("n"), Span::styled(" a vhost for a folder you already have", muted)]).centered(),
                Line::from(vec![key("N"), Span::styled(" a new Laravel or WordPress project", muted)]).centered(),
            ])
            .block(panel("Sites")),
            area,
        );
        return;
    }
    let (table_area, detail_area) = with_detail(area);

    let rows: Vec<Row> = app
        .sites
        .iter()
        .enumerate()
        .map(|(i, site)| {
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
                        Style::new().fg(t.ok),
                    ),
                    false => Span::styled("✗ exited", Style::new().fg(t.bad)),
                })
                .unwrap_or_else(|| Span::styled("-", Style::new().fg(t.faint)));
            let cert = app
                .report
                .as_ref()
                .and_then(|report| report.certs.iter().find(|cert| cert.host == site.host))
                .map(cert_cell)
                .unwrap_or_else(|| Span::styled("-", Style::new().fg(t.faint)));
            Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled(if site.tls { "https://" } else { "http://" }, Style::new().fg(t.faint)),
                    Span::styled(site.host.clone(), Style::new().fg(t.text).bold()),
                ])),
                Cell::from(Line::from(serves_span(app, site))),
                Cell::from(Line::from(dev)),
                Cell::from(Line::from(cert)),
                Cell::from(root).style(Style::new().fg(t.muted)),
            ])
            .style(stripe(i))
        })
        .collect();

    table(
        frame,
        table_area,
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
        .block(panel_counted("Sites", app.sites.len())),
        app.sites.len(),
        app.site_row,
    );

    if let Some(detail_area) = detail_area {
        match app.selected_site() {
            Some(site) => site_detail(frame, app, site, detail_area),
            None => frame.render_widget(panel("Selected"), detail_area),
        }
    }
}

fn site_detail(frame: &mut Frame, app: &App, site: &Site, area: Rect) {
    let t = theme();
    let muted = Style::new().fg(t.muted);
    let mut lines = vec![
        Line::from(Span::styled(site.host.clone(), Style::new().fg(t.text).bold())),
        Line::from(Span::styled(site.url(), Style::new().fg(t.info).underlined())),
        Line::from(""),
        field("serves", serves_span(app, site)),
        field(
            "root",
            Span::styled(site.root.clone().unwrap_or_else(|| "no root directive".into()), muted),
        ),
    ];

    let report = app.report.as_ref();
    match report.and_then(|report| report.certs.iter().find(|cert| cert.host == site.host)) {
        Some(cert) => {
            let colour = match cert.state {
                CertState::Ok => t.ok,
                CertState::Expiring => t.warn,
                _ => t.bad,
            };
            lines.push(field("cert", Span::styled(cert.state.label(), Style::new().fg(colour))));
            if let Some(expires) = &cert.expires {
                lines.push(field("expires", Span::raw(expires.clone())));
            }
            lines.push(field("file", Span::styled(cert.file.clone(), muted)));
        }
        None => lines.push(field("cert", Span::styled("none", muted))),
    }
    match report.and_then(|report| report.dev.iter().find(|dev| dev.host == site.host)) {
        Some(dev) if dev.running => {
            lines.push(field(
                "dev",
                Span::styled(format!("{} (pid {})", dev.command, dev.pid), Style::new().fg(t.ok)),
            ));
            if let Some(url) = &dev.url {
                lines.push(field("", Span::styled(url.clone(), Style::new().fg(t.info))));
            }
        }
        Some(dev) => {
            lines.push(field("dev", Span::styled("exited", Style::new().fg(t.bad))));
            lines.push(field("", Span::styled(dev.log.clone(), muted)));
        }
        None => lines.push(field("dev", Span::styled("not running", muted))),
    }

    lines.push(Line::from(""));
    lines.push(advice("o opens it, v runs its dev server, R renews its certificate."));
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel("Selected").padding(Padding::horizontal(1))),
        area,
    );
}

/// A site's certificate in a few characters: how long it has, or what is wrong.
fn cert_cell(cert: &crate::cert::SiteCert) -> Span<'static> {
    let t = theme();
    let days = cert.days_left.unwrap_or(0);
    match cert.state {
        CertState::Ok if days >= 365 => Span::styled(format!("✓ {}y", days / 365), Style::new().fg(t.ok)),
        CertState::Ok => Span::styled(format!("✓ {days}d"), Style::new().fg(t.ok)),
        CertState::Expiring => Span::styled(format!("! {days}d"), Style::new().fg(t.warn)),
        CertState::Expired => Span::styled("✗ expired", Style::new().fg(t.bad)),
        CertState::NotCovering => Span::styled("✗ name", Style::new().fg(t.bad)),
        CertState::ForeignCa => Span::styled("✗ CA", Style::new().fg(t.bad)),
        CertState::Missing => Span::styled("✗ missing", Style::new().fg(t.bad)),
        CertState::Unreadable => Span::styled("✗ bad", Style::new().fg(t.bad)),
    }
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

fn render_logs(frame: &mut Frame, app: &App, area: Rect) {
    let t = theme();
    let [picker_area, tail_area] =
        Layout::horizontal([Constraint::Length(36), Constraint::Min(30)]).spacing(1).areas(area);

    let items: Vec<ListItem> = app
        .logs
        .iter()
        .map(|file| {
            ListItem::new(Line::from(vec![
                Span::raw(file.label.clone()),
                Span::styled(format!("  {}", logs::human_size(file.size)), Style::new().fg(t.faint)),
            ]))
        })
        .collect();

    let mut state = ListState::default().with_selected(Some(app.log_row));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel_counted("Files", app.logs.len()))
            .highlight_style(t.highlight())
            .highlight_symbol(Line::from(selection_edge()))
            .highlight_spacing(HighlightSpacing::Always),
        picker_area,
        &mut state,
    );
    scrollbar(frame, panel_counted("Files", 0).inner(picker_area), app.logs.len(), app.log_row);

    let Some(view) = &app.log else {
        let message = if app.logs.is_empty() {
            "Nothing has logged yet. Files appear here once a service has run."
        } else {
            "Reading..."
        };
        frame.render_widget(
            Paragraph::new(Line::from(format!(" {message}").fg(t.muted))).block(panel("Log")),
            tail_area,
        );
        return;
    };

    // Show the window ending `log_scroll` lines above the last line -- but
    // never scrolled past a full first page: Home asks for "as far up as
    // there is", which is the top of the file, not an empty pane above it.
    let block = panel(&view.path.display().to_string()).padding(Padding::left(1));
    let inner = block.inner(tail_area);
    let height = inner.height as usize;
    let scroll = app.log_scroll.min(view.lines.len().saturating_sub(height));
    let end = view.lines.len() - scroll;
    let start = end.saturating_sub(height);
    let mut lines: Vec<Line> = view.lines[start..end].iter().map(|line| log_line(line)).collect();
    if start == 0 && view.truncated && !lines.is_empty() {
        lines[0] = Line::from("(earlier lines not shown)".fg(t.muted));
    }
    if lines.is_empty() {
        lines.push(Line::from("(empty)".fg(t.muted)));
    }

    // Following or paused, on the right of the title row.
    let badge = if app.follow {
        Span::styled(" ● following ", Style::new().fg(t.ok))
    } else {
        Span::styled(format!(" ‖ paused, {scroll} lines up "), Style::new().fg(t.warn))
    };
    frame.render_widget(
        Paragraph::new(lines).block(block.title(Line::from(badge).right_aligned())),
        tail_area,
    );
    // The thumb sits where the window is: at the bottom while following.
    let max = view.lines.len().saturating_sub(height);
    if max > 0 {
        let mut state = ScrollbarState::new(max).position(start);
        frame.render_stateful_widget(thumb_only(), inner, &mut state);
    }
}

/// Colour a log line by the severity it names, as nginx, PHP, and MariaDB spell
/// it. Only a hint: the text itself is shown unchanged.
fn log_line(line: &str) -> Line<'_> {
    let t = theme();
    let lower = line.to_ascii_lowercase();
    let mentions = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    let colour = if mentions(&["[error]", "[crit]", "[alert]", "[emerg]", "fatal error", "[error "]) {
        Some(t.bad)
    } else if mentions(&["[warn]", "[warning]", "warning:"]) {
        Some(t.warn)
    } else if mentions(&["[notice]", "[note]", "[info]"]) {
        Some(t.muted)
    } else {
        None
    };
    match colour {
        Some(colour) => Line::from(Span::styled(line, Style::new().fg(colour))),
        None => Line::from(line),
    }
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

/// The `42` of `downloading  42%  12.0 MB / 28.6 MB`, when progress names one.
fn percent_in(text: &str) -> Option<usize> {
    let at = text.find('%')?;
    let digits: String = text[..at]
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok().filter(|percent| *percent <= 100)
}

/// A status line -- what is running, or how the last action went -- over a
/// bar of the keys this pane takes.
fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let t = theme();
    let [note_area, keys_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    let note = match (&app.busy, &app.note) {
        (Some(busy), _) => {
            let mut spans = vec![
                Span::styled(format!(" {} ", spinner()), Style::new().fg(t.accent).bold()),
                Span::styled(format!("{busy}..."), Style::new().fg(t.text).bold()),
            ];
            if let Some(progress) = &app.progress {
                if let Some(percent) = percent_in(progress) {
                    spans.push(Span::raw("  "));
                    spans.extend(gauge(percent, 100, 16, t.accent));
                }
                spans.push(Span::styled(format!("  {progress}"), Style::new().fg(t.info)));
            }
            Line::from(spans)
        }
        (None, Some(note)) if !note.is_stale() => {
            let (icon, colour) = if note.failed { ("✗", t.bad) } else { ("✓", t.ok) };
            Line::from(vec![
                Span::styled(format!(" {icon} "), Style::new().fg(colour).bold()),
                Span::styled(note.text.clone(), Style::new().fg(colour)),
            ])
        }
        _ => Line::from(vec![
            Span::styled(" ◆ ", Style::new().fg(t.border)),
            Span::styled("ready", Style::new().fg(t.faint)),
        ]),
    };
    frame.render_widget(Paragraph::new(note), note_area);

    // In the order they are worth keeping: on a narrow screen the hints drop
    // from the end, and the last two -- help and quit -- always stay.
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
            ("A", "autostart"),
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
            ("R", "renew cert"),
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
    frame.render_widget(
        Paragraph::new(key_line(keys, keys_area.width as usize)).style(Style::new().bg(t.panel)),
        keys_area,
    );
}

/// As many of `keys` as fit in `width`. The last two are how to get help and
/// how to leave, so they are kept and the others give way from the end.
fn key_line(keys: &[(&str, &str)], width: usize) -> Line<'static> {
    // The key's block, its description, and the gap after: ` s start  `.
    let cost = |(name, what): &(&str, &str)| name.chars().count() + 2 + what.chars().count() + 2;
    let pinned = keys.len().min(2);
    let (rest, tail) = keys.split_at(keys.len() - pinned);
    let tail_cost: usize = tail.iter().map(cost).sum::<usize>() + 1;
    let mut shown = rest.len();
    while shown > 0 && rest[..shown].iter().map(cost).sum::<usize>() + tail_cost > width {
        shown -= 1;
    }

    let mut spans = vec![Span::raw(" ")];
    for (i, (name, what)) in rest[..shown].iter().chain(tail).enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(key(name));
        spans.push(Span::styled(what.to_string(), Style::new().fg(theme().muted)));
    }
    Line::from(spans)
}

// ---------------------------------------------------------------------------
// Modals
// ---------------------------------------------------------------------------

fn render_modal(frame: &mut Frame, app: &App, modal: &Modal) {
    let t = theme();
    match modal {
        Modal::Help => render_help(frame),
        Modal::Output { title, lines, failed } => render_output(frame, title, lines, *failed),
        Modal::Confirm { question, .. } => {
            let mut text = Text::from(question.as_str());
            text.push_line(Line::from(""));
            // Enter confirms as well as y, and this says so: a line promising
            // that every other key cancels was not true of the one key most
            // likely to be pressed without reading.
            text.push_line(Line::from(vec![
                key("y"),
                Span::styled("or ", Style::new().fg(t.muted)),
                key("Enter"),
                Span::styled("does it     any other key cancels", Style::new().fg(t.muted)),
            ]));
            let area = centred(frame.area(), 64, 9);
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: false })
                    .block(modal_block("⚠ Confirm", t.warn).padding(Padding::new(1, 1, 1, 0))),
                area,
            );
        }
        Modal::NewSite(form) => {
            let versions = app.php_versions();
            let php_label = if !form.proxy.is_empty() {
                "— (proxied)".to_string()
            } else if form.php_index == 0 {
                "Auto-detect (composer.json / default)".to_string()
            } else if let Some((_, name)) = versions.get(form.php_index - 1) {
                name.clone()
            } else {
                "Auto-detect".to_string()
            };

            let f_style = |idx| {
                if form.active_field == idx {
                    Style::new().fg(t.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(t.text)
                }
            };
            let label = |idx, text: &'static str| {
                let marker = if form.active_field == idx { "› " } else { "  " };
                Span::styled(format!("{marker}{text:<17}"), f_style(idx))
            };
            let check = |on: bool| if on { "[✓] " } else { "[ ] " };

            let text = Text::from(vec![
                Line::from(vec![label(0, "Hostname"), Span::styled(&form.host, f_style(0)), cursor(form.active_field == 0)]),
                Line::from(vec![
                    label(1, "Project path"),
                    if form.path.is_empty() {
                        Span::styled("projects/<host>  (default)", Style::new().fg(t.muted))
                    } else {
                        Span::styled(&form.path, f_style(1))
                    },
                    cursor(form.active_field == 1),
                ]),
                Line::from(vec![Span::raw(" ".repeat(19)), button(form.active_field == 2, " Browse for a folder... ")]),
                Line::from(vec![
                    label(3, "PHP version"),
                    Span::styled("‹ ", Style::new().fg(t.muted)),
                    Span::styled(&php_label, f_style(3)),
                    Span::styled(" ›", Style::new().fg(t.muted)),
                ]),
                Line::from(vec![
                    label(4, "Proxy port"),
                    if form.proxy.is_empty() {
                        Span::styled("none -- serve PHP  (digits: forward to localhost:<port>)", Style::new().fg(t.muted))
                    } else {
                        Span::styled(format!("localhost:{}", form.proxy), f_style(4))
                    },
                    cursor(form.active_field == 4),
                ]),
                Line::from(vec![
                    label(5, "Hosts file"),
                    Span::styled(
                        format!(
                            "{}{}",
                            check(form.update_hosts),
                            if form.update_hosts { "add it to the Windows hosts file" } else { "leave the hosts file alone" }
                        ),
                        f_style(5),
                    ),
                ]),
                Line::from(vec![
                    label(6, "TLS certificate"),
                    Span::styled(
                        format!(
                            "{}{}",
                            check(form.issue_tls),
                            if form.issue_tls { "issue one with mkcert" } else { "issue none" }
                        ),
                        f_style(6),
                    ),
                ]),
                Line::from(""),
                Line::from(vec![Span::raw(" ".repeat(19)), button(form.active_field == 7, "  Create the site  ")]),
                Line::from(""),
                // What the form really takes: j and k are letters of a
                // hostname here, not movement.
                Line::from(vec![
                    key("Tab ↑↓"),
                    Span::styled("fields  ", Style::new().fg(t.muted)),
                    key("← → Space"),
                    Span::styled("change  ", Style::new().fg(t.muted)),
                    key("Enter"),
                    Span::styled("create  ", Style::new().fg(t.muted)),
                    key("Esc"),
                    Span::styled("cancel", Style::new().fg(t.muted)),
                ]),
            ]);
            popup(frame, "New site", text, 80, 13);
        }
        Modal::NewProject(form) => {
            let versions = app.php_versions();
            let php_label = match form.php_index.checked_sub(1).and_then(|i| versions.get(i)) {
                Some((_, name)) => name.clone(),
                None => "php\\current".to_string(),
            };
            let style = |idx| match form.active_field == idx {
                true => Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
                false => Style::new().fg(t.text),
            };
            let label = |idx, text: &'static str| {
                let marker = if form.active_field == idx { "› " } else { "  " };
                Span::styled(format!("{marker}{text:<12}"), style(idx))
            };
            let choice = |idx, text: String| {
                vec![
                    Span::styled("‹ ", Style::new().fg(t.muted)),
                    Span::styled(text, style(idx)),
                    Span::styled(" ›", Style::new().fg(t.muted)),
                ]
            };
            let host = crate::create::names(form.name.trim()).map(|(host, _)| host).unwrap_or_default();
            let mut lines = vec![
                Line::from([vec![label(0, "Kind")], choice(0, form.kind().label().to_string())].concat()),
                Line::from(vec![
                    label(1, "Name"),
                    Span::styled(form.name.clone(), style(1)),
                    cursor(form.active_field == 1),
                    Span::styled(
                        if host.is_empty() { String::new() } else { format!("   https://{host}") },
                        Style::new().fg(t.info),
                    ),
                ]),
                Line::from([vec![label(2, "PHP")], choice(2, php_label)].concat()),
                Line::from([vec![label(3, "Database")], choice(3, form.database().label().to_string())].concat()),
                Line::from(""),
                Line::from(vec![Span::raw(" ".repeat(14)), button(form.active_field == 4, "  Create the project  ")]),
            ];
            lines.push(match &form.error {
                Some(error) => Line::from(Span::styled(error.clone(), Style::new().fg(t.bad))),
                None => Line::from(Span::styled(
                    "Into projects\\<name>, never over a folder with content. Composer's output goes to logs\\.",
                    Style::new().fg(t.muted),
                )),
            });
            lines.push(Line::from(vec![
                key("Tab ↑↓"),
                Span::styled("fields  ", Style::new().fg(t.muted)),
                key("← →"),
                Span::styled("choose  ", Style::new().fg(t.muted)),
                key("Enter"),
                Span::styled("create  ", Style::new().fg(t.muted)),
                key("Esc"),
                Span::styled("cancel", Style::new().fg(t.muted)),
            ]));
            popup(frame, "New project (devcrate new)", Text::from(lines), 80, 12);
        }
        Modal::PhpPicker { purpose, index } => {
            let versions = app.php_versions();
            let title = match purpose {
                PhpPurpose::Cli => "Which PHP should the command line run?".to_string(),
                PhpPurpose::Site(host) => format!("Serve {host} with"),
                PhpPurpose::NewSite(host) => format!("Create {host} with"),
            };
            let items: Vec<ListItem> = versions
                .iter()
                .map(|(id, name)| {
                    ListItem::new(Line::from(vec![
                        Span::raw(name.clone()),
                        Span::styled(format!("   {id}"), Style::new().fg(t.muted)),
                    ]))
                })
                .collect();
            picker(frame, &title, items, *index, "Enter choose · Esc close");
        }
        Modal::InstallPicker { index } => {
            let items: Vec<ListItem> = app
                .install_options
                .iter()
                .map(|offering| ListItem::new(offering.label.clone()))
                .collect();
            let title = if app.offerings_loaded {
                "Install a service or runtime".to_string()
            } else {
                format!("Install a service or runtime  {} reading catalogues", spinner())
            };
            picker(frame, &title, items, *index, "Enter download and install · Esc close");
        }
        Modal::UninstallPicker { index, items } => {
            let items: Vec<ListItem> = items.iter().map(|item| ListItem::new(item.label.clone())).collect();
            picker(frame, "Uninstall", items, *index, "Enter asks first · Esc close");
        }
        Modal::ExtPicker { php_name, index, rows, .. } => {
            let items: Vec<ListItem> = rows.iter().map(ext_row).collect();
            let title = format!("{php_name} extensions");
            picker(frame, &title, items, *index, "Enter or Space switch · Esc close");
        }
    }
}

/// The result of the last job. Its ending is what matters -- the step that
/// failed, the line that says where a project ended up -- so when there is
/// more than fits, the start gives way and says how much of it did.
fn render_output(frame: &mut Frame, title: &str, lines: &[String], failed: bool) {
    let t = theme();
    let screen = frame.area();
    let width = screen.width.saturating_sub(4).clamp(20, 96);
    let inner_width = width.saturating_sub(4).max(1) as usize;
    let room = screen.height.saturating_sub(6).max(3) as usize;

    // A result line can carry a line break of its own ("NOT DONE ...", then
    // the command that finishes it).
    let all: Vec<&str> = lines.iter().flat_map(|line| line.lines()).collect();
    let mut shown: Vec<&str> = Vec::new();
    let mut used = 0;
    for line in all.iter().rev() {
        let rows = wrapped_rows(line.chars().count(), inner_width);
        if used + rows > room && !shown.is_empty() {
            break;
        }
        used += rows;
        shown.push(line);
    }
    shown.reverse();
    let hidden = all.len() - shown.len();

    // The lines that say what went wrong, where a result marks them; a
    // failure that marks none is one error message, and all of it is the news.
    let marked = |line: &str| {
        ["FAILED", "NOT DONE", "stopped at", "did not", "not started again"].iter().any(|word| line.contains(word))
    };
    let any_marked = shown.iter().any(|line| marked(line));
    let mut text: Vec<Line> = Vec::new();
    if hidden > 0 {
        text.push(Line::from(Span::styled(
            format!("... {hidden} earlier line(s) not shown"),
            Style::new().fg(t.muted),
        )));
    }
    text.extend(shown.iter().map(|line| {
        let colour = if marked(line) || (failed && !any_marked) { t.bad } else { t.text };
        Line::from(Span::styled(line.to_string(), Style::new().fg(colour)))
    }));

    let (icon, colour) = if failed { ("✗", t.bad) } else { ("✓", t.ok) };
    let height = (used + usize::from(hidden > 0) + 2) as u16;
    let area = centred(screen, width, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(
            modal_block(&format!("{icon} {title}"), colour)
                .title_bottom(Line::from(Span::styled(" any key closes ", Style::new().fg(t.muted))).right_aligned())
                .padding(Padding::horizontal(1)),
        ),
        area,
    );
}

fn ext_row(row: &ExtRow) -> ListItem<'static> {
    use crate::php_ext::Source;
    let t = theme();
    match row {
        ExtRow::Install(name) => ListItem::new(Line::from(vec![
            Span::styled("get ", Style::new().fg(t.accent)),
            Span::raw(format!("{name:<14}")),
            Span::styled("not installed -- Enter downloads a matching build", Style::new().fg(t.muted)),
        ])),
        ExtRow::Ext(ext) => {
            let (mark, colour) = match ext.enabled {
                Some(_) => ("on  ", t.ok),
                None => ("off ", t.faint),
            };
            let note = match &ext.enabled {
                Some(Source::Ini(line)) => format!("php.ini line {line} -- yours, not switched from here"),
                Some(Source::Block) => "devcrate".to_string(),
                None => String::new(),
            };
            ListItem::new(Line::from(vec![
                Span::styled(mark, Style::new().fg(colour)),
                Span::raw(format!("{:<14}", ext.name)),
                Span::styled(note, Style::new().fg(t.muted)),
            ]))
        }
    }
}

fn picker(frame: &mut Frame, title: &str, items: Vec<ListItem>, index: usize, hint: &str) {
    let t = theme();
    if items.is_empty() {
        return popup(frame, title, Text::from(" Nothing is installed.".fg(t.muted)), 48, 3);
    }
    let len = items.len();
    // Never taller than the screen leaves room for; the list scrolls inside.
    let height = ((len + 2).max(3) as u16).min(frame.area().height.saturating_sub(2).max(3));
    // Wide enough for the longest entry, the title, and the hint, plus the
    // marker and the frame; never narrower than the 48 the short lists use.
    let widest = items.iter().map(ListItem::width).max().unwrap_or(0);
    let width = (widest + 6)
        .max(title.chars().count() + 6)
        .max(hint.chars().count() + 6)
        .max(48) as u16;
    let area = centred(frame.area(), width, height);
    frame.render_widget(Clear, area);
    let block = modal_block(title, t.accent)
        .title_bottom(Line::from(Span::styled(format!(" {hint} "), Style::new().fg(t.muted))).right_aligned());
    let inner = block.inner(area);
    let mut state = ListState::default().with_selected(Some(index));
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(t.highlight())
            .highlight_symbol(Line::from(selection_edge()))
            .highlight_spacing(HighlightSpacing::Always),
        area,
        &mut state,
    );
    scrollbar(frame, inner, len, index);
}

/// Every key, in two columns where the screen is wide enough for them: the
/// whole of it then fits a terminal twenty rows tall.
fn render_help(frame: &mut Frame) {
    let t = theme();
    let section = |name: &'static str| Line::from(name.fg(t.accent).bold());
    let row = |keys: &'static str, what: &'static str| {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{keys:<12}"), Style::new().fg(t.info).bold()),
            Span::styled(what, Style::new().fg(t.text)),
        ])
    };
    let left = vec![
        section("Anywhere"),
        row("1 2 3 Tab", "switch pane"),
        row("↑ ↓ j k", "move"),
        row("r", "rescan now"),
        row("?  q", "this help / quit"),
        Line::from(""),
        section("Services"),
        row("s x t", "start / stop / restart one"),
        row("S X T", "...the whole stack, in order"),
        row("i  U", "install / uninstall a runtime"),
        row("u", "CLI PHP (php\\current)"),
        row("e", "PHP extensions, Xdebug too"),
        row("A", "autostart at logon on / off"),
    ];
    let right = vec![
        section("Sites"),
        row("n", "new vhost"),
        row("N", "new Laravel / WordPress project"),
        row("p", "change PHP version"),
        row("v", "dev server (npm run dev) on / off"),
        row("R", "issue its certificate again"),
        row("o e c", "open in the browser, folder, shell"),
        row("d", "delete the vhost conf"),
        Line::from(""),
        section("Logs"),
        row("f", "follow on / off"),
        row("PgUp PgDn", "scroll"),
        row("Home / End", "top / bottom"),
    ];

    let screen = frame.area();
    let two_columns = screen.width >= 100;
    let (width, rows) = match two_columns {
        true => (100, left.len().max(right.len())),
        false => (54, left.len() + 1 + right.len()),
    };
    let area = centred(screen, width, rows as u16 + 2);
    frame.render_widget(Clear, area);
    let block = modal_block("Keys", t.accent)
        .title_bottom(Line::from(Span::styled(" any key closes ", Style::new().fg(t.muted))).right_aligned())
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if two_columns {
        let [left_area, right_area] = Layout::horizontal([Constraint::Ratio(1, 2); 2]).areas(inner);
        frame.render_widget(Paragraph::new(left), left_area);
        frame.render_widget(Paragraph::new(right), right_area);
    } else {
        let mut all = left;
        all.push(Line::from(""));
        all.extend(right);
        frame.render_widget(Paragraph::new(all), inner);
    }
}

fn popup(frame: &mut Frame, title: &str, text: Text<'_>, width: u16, height: u16) {
    let area = centred(frame.area(), width, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(modal_block(title, theme().accent).padding(Padding::horizontal(1))),
        area,
    );
}

/// A box of at most `width` x `height`, centred, never larger than the screen.
fn centred(area: Rect, width: u16, height: u16) -> Rect {
    area.centered(Constraint::Length(width.min(area.width)), Constraint::Length(height.min(area.height)))
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

/// A key name in a small block of its own, as the footer, help, and forms
/// show it. The block's right padding is the gap before what the key does.
fn key(name: &str) -> Span<'static> {
    let t = theme();
    Span::styled(format!(" {name} "), t.block(t.key_fg, t.key_bg).bold())
}

/// The blinking bar after the text of the field being typed into.
fn cursor(active: bool) -> Span<'static> {
    match active {
        true => Span::styled("▏", Style::new().fg(theme().accent).add_modifier(Modifier::SLOW_BLINK)),
        false => Span::raw(""),
    }
}

/// A form's button: filled in the accent when it has the focus.
fn button(focused: bool, text: &'static str) -> Span<'static> {
    let t = theme();
    match focused {
        true => Span::styled(text, t.block(t.on_accent, t.accent).bold()),
        false => Span::styled(text, Style::new().fg(t.accent).bg(t.selected)),
    }
}

/// A pane: a block of the panel's colour, its name on the first row. No lines
/// around it -- the change of background is the edge.
fn panel(title: &str) -> Block<'static> {
    let t = theme();
    Block::new()
        .style(Style::new().bg(t.panel).fg(t.text))
        .title(Line::from(Span::styled(format!(" {title} "), Style::new().fg(t.accent).bold())))
}

/// ...and how many rows it holds, beside the name.
fn panel_counted(title: &str, count: usize) -> Block<'static> {
    let t = theme();
    Block::new().style(Style::new().bg(t.panel).fg(t.text)).title(Line::from(vec![
        Span::styled(format!(" {title} "), Style::new().fg(t.accent).bold()),
        Span::styled(format!("{count} "), Style::new().fg(t.faint)),
    ]))
}

/// A popup's frame: lifted off the dashboard by its own background, with the
/// frame in the colour of what it is about -- the accent, amber for a
/// question, red for a failure.
fn modal_block(title: &str, colour: Color) -> Block<'static> {
    let t = theme();
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(colour))
        .style(Style::new().bg(t.raised).fg(t.text))
        .title(Line::from(Span::styled(format!(" {title} "), Style::new().fg(colour).bold())))
}

fn table_header<const N: usize>(names: [&'static str; N]) -> Row<'static> {
    let t = theme();
    Row::new(names).style(Style::new().fg(t.muted).bg(t.raised).add_modifier(Modifier::BOLD))
}

/// Every other row a shade darker, so a wide row is easy to follow across.
fn stripe(row: usize) -> Style {
    let t = theme();
    Style::new().bg(if row % 2 == 1 { t.stripe } else { t.panel })
}

/// The accent edge in front of the selected row.
fn selection_edge() -> Span<'static> {
    Span::styled("▌ ", Style::new().fg(theme().accent))
}

/// A table in its panel, with the selection drawn and -- when there are more
/// rows than fit -- a scrollbar down its right edge.
fn table(frame: &mut Frame, area: Rect, table: Table, len: usize, selected: usize) {
    let t = theme();
    let inner = panel("").inner(area);
    let mut state = TableState::default().with_selected(Some(selected));
    frame.render_stateful_widget(
        table
            .column_spacing(2)
            .row_highlight_style(t.highlight())
            .highlight_symbol(selection_edge())
            .highlight_spacing(HighlightSpacing::Always),
        area,
        &mut state,
    );
    // Below the heading row.
    let rows = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    scrollbar(frame, rows, len, selected);
}

/// A thumb-only scrollbar down the right edge of `area`, drawn only when `len`
/// rows do not fit in it.
fn scrollbar(frame: &mut Frame, area: Rect, len: usize, position: usize) {
    if len <= area.height as usize || area.height == 0 {
        return;
    }
    let mut state = ScrollbarState::new(len.saturating_sub(1)).position(position);
    frame.render_stateful_widget(thumb_only(), area, &mut state);
}

fn thumb_only() -> Scrollbar<'static> {
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(None)
        .thumb_symbol("▐")
        .thumb_style(Style::new().fg(theme().faint))
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
