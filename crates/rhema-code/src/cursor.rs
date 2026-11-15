//! The terminal caret's shape: the stored look-and-feel choice, its
//! cycle, and the crossterm command each shape writes.

use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::SetCursorStyle;

/// Whether this session has written DECSCUSR at all. A default session
/// leaves it false, so exit and panics have nothing to undo.
static TOUCHED: AtomicBool = AtomicBool::new(false);

/// The shape the input's caret takes. Default writes no DECSCUSR, so
/// the cursor the shell left in place stays (a line at a prompt under
/// shell integration); the other three ask the terminal for a specific
/// caret.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorStyle {
    /// The terminal's own cursor, untouched.
    #[default]
    Default,
    /// A steady block.
    Block,
    /// A steady vertical bar (`|`).
    Line,
    /// A steady underscore (`_`).
    Underline,
}

impl CursorStyle {
    /// Every style, in the Appearance page's cycle order.
    pub const ALL: &'static [CursorStyle] = &[
        CursorStyle::Default,
        CursorStyle::Block,
        CursorStyle::Line,
        CursorStyle::Underline,
    ];

    /// Parses a stored config value; `None` for no value and unknown
    /// spellings alike, the caller reads that as the default.
    pub fn from_stored(value: Option<&str>) -> Option<Self> {
        match value {
            Some("default") => Some(Self::Default),
            Some("block") => Some(Self::Block),
            Some("line") | Some("bar") => Some(Self::Line),
            Some("underline") | Some("underscore") => Some(Self::Underline),
            _ => None,
        }
    }

    /// The config value this style persists as, the inverse of
    /// [`Self::from_stored`] for the canonical spelling.
    pub fn stored_name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Block => "block",
            Self::Line => "line",
            Self::Underline => "underline",
        }
    }

    /// The label the settings screen draws.
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Block => "Block",
            Self::Line => "Line",
            Self::Underline => "Underline",
        }
    }

    /// The next style in the cycle, wrapping at the end.
    pub fn next(self) -> Self {
        match self {
            Self::Default => Self::Block,
            Self::Block => Self::Line,
            Self::Line => Self::Underline,
            Self::Underline => Self::Default,
        }
    }

    /// The DECSCUSR command this style writes. The explicit shapes are
    /// steady: a caret that blinks away mid-word is harder to find, and
    /// the terminal's own shape is one cycle away for anyone who wants
    /// its blink back.
    pub fn set_cursor_style(self) -> SetCursorStyle {
        match self {
            Self::Default => SetCursorStyle::DefaultUserShape,
            Self::Block => SetCursorStyle::SteadyBlock,
            Self::Line => SetCursorStyle::SteadyBar,
            Self::Underline => SetCursorStyle::SteadyUnderScore,
        }
    }

    /// Writes the style to the terminal, best effort: a terminal
    /// without DECSCUSR ignores the sequence and keeps its own caret.
    ///
    /// Default writes nothing while the session has not changed the
    /// cursor. That is the point: the shell's own shape, a line under
    /// shell integration, survives startup. Once a non-default choice
    /// has landed, switching back to Default writes DECSCUSR 0, the
    /// best reset a terminal offers.
    pub fn apply(self) {
        let Some(command) = self.command(TOUCHED.load(Ordering::Relaxed)) else {
            return;
        };
        use std::io::Write;
        let _ = crossterm::execute!(std::io::stdout(), command);
        let _ = std::io::stdout().flush();
        TOUCHED.store(true, Ordering::Relaxed);
    }

    /// The command to write, if any. Default is a no-op until the
    /// session has written a shape; after that it resets, because the
    /// terminal's own shape is not recoverable once overwritten.
    fn command(self, touched: bool) -> Option<SetCursorStyle> {
        if self == Self::Default && !touched {
            return None;
        }
        Some(self.set_cursor_style())
    }

    /// Puts the terminal cursor back only when this session wrote
    /// DECSCUSR. A default session never did, so it leaves the shell's
    /// cursor exactly as it found it.
    pub fn restore() {
        if !TOUCHED.swap(false, Ordering::Relaxed) {
            return;
        }
        use std::io::Write;
        let _ = crossterm::execute!(std::io::stdout(), SetCursorStyle::DefaultUserShape);
        let _ = std::io::stdout().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_spellings_round_trip() {
        for style in CursorStyle::ALL {
            assert_eq!(
                CursorStyle::from_stored(Some(style.stored_name())),
                Some(*style)
            );
        }
        assert_eq!(CursorStyle::from_stored(None), None);
        assert_eq!(CursorStyle::from_stored(Some("square")), None);
        // The long spelling reads the same as the label.
        assert_eq!(
            CursorStyle::from_stored(Some("underline")),
            Some(CursorStyle::Underline)
        );
    }

    #[test]
    fn the_cycle_wraps_through_every_style() {
        assert_eq!(CursorStyle::Default.next(), CursorStyle::Block);
        assert_eq!(CursorStyle::Block.next(), CursorStyle::Line);
        assert_eq!(CursorStyle::Line.next(), CursorStyle::Underline);
        assert_eq!(CursorStyle::Underline.next(), CursorStyle::Default);
    }

    #[test]
    fn default_writes_nothing_until_a_shape_was_set() {
        assert_eq!(CursorStyle::Default.command(false), None);
        assert_eq!(
            CursorStyle::Default.command(true),
            Some(SetCursorStyle::DefaultUserShape)
        );
        assert_eq!(
            CursorStyle::Block.command(false),
            Some(SetCursorStyle::SteadyBlock)
        );
        assert_eq!(
            CursorStyle::Block.command(true),
            Some(SetCursorStyle::SteadyBlock)
        );
    }

    #[test]
    fn each_style_maps_to_its_decsusr_shape() {
        assert_eq!(
            CursorStyle::Default.set_cursor_style(),
            SetCursorStyle::DefaultUserShape
        );
        assert_eq!(
            CursorStyle::Block.set_cursor_style(),
            SetCursorStyle::SteadyBlock
        );
        assert_eq!(
            CursorStyle::Line.set_cursor_style(),
            SetCursorStyle::SteadyBar
        );
        assert_eq!(
            CursorStyle::Underline.set_cursor_style(),
            SetCursorStyle::SteadyUnderScore
        );
    }
}
