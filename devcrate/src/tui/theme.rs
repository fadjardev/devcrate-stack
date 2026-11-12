//! The dashboard's colours, in one place.
//!
//! Built the way ratatui's own `demo2` and `table` examples build theirs: a
//! struct of named roles filled from the Tailwind palette that ratatui ships
//! (`style::palette::tailwind`). Slate makes the neutrals, indigo is the one
//! accent, and emerald / amber / rose say good / careful / wrong.
//!
//! Every cell of the frame is painted, so the terminal's own theme -- light or
//! dark -- decides nothing. The exception is `NO_COLOR` (<https://no-color.org>):
//! with it set, every role is the terminal's default colour and the few places
//! that need to stand out -- the selected row, the active tab, a key -- are
//! drawn in reverse video instead.

use std::sync::OnceLock;

use ratatui::style::palette::tailwind::{AMBER, EMERALD, INDIGO, ROSE, SKY, SLATE};
use ratatui::style::{Color, Modifier, Style};

pub struct Theme {
    /// The frame itself, around and between panels.
    pub bg: Color,
    /// A panel: each pane, card, and the active tab, which joins its pane.
    pub panel: Color,
    /// Every other row of a table, a shade below the panel.
    pub stripe: Color,
    /// A table's heading row and a popup -- what sits on top.
    pub raised: Color,
    /// The selected row: a tinted bar rather than reverse video, so the row's
    /// own colours (a red `crashed`, a green port) survive being selected.
    pub selected: Color,
    /// Rules and the empty part of a bar.
    pub border: Color,
    pub text: Color,
    /// Labels, paths, descriptions.
    pub muted: Color,
    /// What is there but not worth reading: an unselected tab's number.
    pub faint: Color,
    /// Where the keyboard is: the active tab, a pane's title, a popup's frame.
    pub accent: Color,
    /// Text drawn on the accent.
    pub on_accent: Color,
    /// Things to read rather than act on: a URL, a PHP version.
    pub info: Color,
    pub ok: Color,
    pub warn: Color,
    pub bad: Color,
    /// A key name's little block in the footer and the forms.
    pub key_bg: Color,
    pub key_fg: Color,
    /// No colour at all: lean on reverse video where colour would have done.
    pub mono: bool,
}

const DARK: Theme = Theme {
    bg: SLATE.c950,
    panel: SLATE.c900,
    stripe: Color::Rgb(22, 32, 52),
    raised: SLATE.c800,
    selected: INDIGO.c950,
    border: SLATE.c700,
    text: SLATE.c200,
    muted: SLATE.c400,
    faint: SLATE.c600,
    accent: INDIGO.c400,
    on_accent: SLATE.c950,
    info: SKY.c400,
    ok: EMERALD.c400,
    warn: AMBER.c400,
    bad: ROSE.c400,
    key_bg: SLATE.c700,
    key_fg: SLATE.c100,
    mono: false,
};

const MONO: Theme = Theme {
    bg: Color::Reset,
    panel: Color::Reset,
    stripe: Color::Reset,
    raised: Color::Reset,
    selected: Color::Reset,
    border: Color::Reset,
    text: Color::Reset,
    muted: Color::Reset,
    faint: Color::Reset,
    accent: Color::Reset,
    on_accent: Color::Reset,
    info: Color::Reset,
    ok: Color::Reset,
    warn: Color::Reset,
    bad: Color::Reset,
    key_bg: Color::Reset,
    key_fg: Color::Reset,
    mono: true,
};

/// The theme for this run: decided once, from `NO_COLOR`.
pub fn theme() -> &'static Theme {
    static THEME: OnceLock<&'static Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        // Any non-empty value counts, as no-color.org asks.
        match std::env::var_os("NO_COLOR") {
            Some(value) if !value.is_empty() => &MONO,
            _ => &DARK,
        }
    })
}

impl Theme {
    /// Text on a coloured block: a key, the brand, the active tab, a focused
    /// button. Reverse video when there is no colour to make the block from.
    pub fn block(&self, fg: Color, bg: Color) -> Style {
        match self.mono {
            true => Style::new().add_modifier(Modifier::REVERSED),
            false => Style::new().fg(fg).bg(bg),
        }
    }

    /// The selected row of a table or a list.
    pub fn highlight(&self) -> Style {
        match self.mono {
            true => Style::new().add_modifier(Modifier::REVERSED),
            false => Style::new().bg(self.selected).add_modifier(Modifier::BOLD),
        }
    }
}
