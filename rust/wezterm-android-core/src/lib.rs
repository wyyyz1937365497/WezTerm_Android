use std::fmt::Write;
use std::sync::Arc;
use wezterm_term::color::{ColorAttribute, ColorPalette, SrgbaTuple};
use wezterm_term::{CellAttributes, Intensity, Terminal, TerminalConfiguration, TerminalSize};

pub const UPSTREAM_WEZTERM_REVISION: &str = "d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b";

const IDLE_CAT_ART: &[&str] = &[
    "      ▄▄▄         ▄▄▄      ",
    "     █▀  ▀▄▄▄▄▄▀  ▀█     ",
    "    █               █    ",
    "   █                 █   ",
    "   █    ▀       ▀    █   ",
    "   █        ▄        █   ",
    " ▀▀█      ▀███▀      █▀▀ ",
    "    ▀▄             ▄▀    ",
    "      ▀▄▄▄▄▄▄▄▄▄▄▄▀      ",
];

/// Build the ANSI payload for the disconnected screen. Keeping the artwork in
/// the terminal stream means it is rendered by the same WezTerm cell, shaping,
/// and glyph-atlas path as a remote session rather than by an Android overlay.
pub fn idle_cat_ansi(columns: usize, rows: usize) -> Vec<u8> {
    let columns = columns.max(1);
    let rows = rows.max(1);
    let visible_height = IDLE_CAT_ART.len().min(rows);
    let top = rows.saturating_sub(visible_height) / 2;
    let mut ansi = String::from("\x1b[0m\x1b[48;2;0;0;0m\x1b[2J\x1b[38;2;255;183;77m");

    for (index, source_line) in IDLE_CAT_ART.iter().take(visible_height).enumerate() {
        let line: String = source_line.chars().take(columns).collect();
        let line_width = line.chars().count();
        let left = columns.saturating_sub(line_width) / 2;
        write!(ansi, "\x1b[{};{}H{}", top + index + 1, left + 1, line)
            .expect("writing to a String cannot fail");
    }
    ansi.push_str("\x1b[0m");
    ansi.into_bytes()
}

/// Encode the Android key-code subset exposed by the in-app and hardware
/// keyboards into the byte sequences expected by an xterm-compatible PTY.
/// Keeping this pure makes the mobile input contract host-testable.
pub fn android_key_bytes(key_code: i32, unicode_code_point: i32, meta_state: i32) -> Vec<u8> {
    const META_ALT_ON: i32 = 0x02;
    const META_CTRL_ON: i32 = 0x1000;
    let special: Option<&[u8]> = match key_code {
        19 => Some(b"\x1b[A"),    // DPAD_UP
        20 => Some(b"\x1b[B"),    // DPAD_DOWN
        21 => Some(b"\x1b[D"),    // DPAD_LEFT
        22 => Some(b"\x1b[C"),    // DPAD_RIGHT
        61 => Some(b"\t"),        // TAB
        66 => Some(b"\r"),        // ENTER
        67 => Some(b"\x7f"),      // DEL / backspace
        92 => Some(b"\x1b[5~"),   // PAGE_UP
        93 => Some(b"\x1b[6~"),   // PAGE_DOWN
        111 => Some(b"\x1b"),     // ESCAPE
        112 => Some(b"\x1b[3~"),  // FORWARD_DEL
        122 => Some(b"\x1b[H"),   // MOVE_HOME
        123 => Some(b"\x1b[F"),   // MOVE_END
        124 => Some(b"\x1b[2~"),  // INSERT
        131 => Some(b"\x1bOP"),   // F1
        132 => Some(b"\x1bOQ"),   // F2
        133 => Some(b"\x1bOR"),   // F3
        134 => Some(b"\x1bOS"),   // F4
        135 => Some(b"\x1b[15~"), // F5
        136 => Some(b"\x1b[17~"), // F6
        137 => Some(b"\x1b[18~"), // F7
        138 => Some(b"\x1b[19~"), // F8
        139 => Some(b"\x1b[20~"), // F9
        140 => Some(b"\x1b[21~"), // F10
        141 => Some(b"\x1b[23~"), // F11
        142 => Some(b"\x1b[24~"), // F12
        _ => None,
    };
    let mut bytes = special.map(ToOwned::to_owned).unwrap_or_default();
    if bytes.is_empty() {
        if unicode_code_point <= 0 {
            return bytes;
        }
        let Ok(codepoint) = u32::try_from(unicode_code_point) else {
            return bytes;
        };
        let Some(character) = char::from_u32(codepoint) else {
            return bytes;
        };
        if meta_state & META_CTRL_ON != 0 && character.is_ascii() {
            let upper = character.to_ascii_uppercase() as u8;
            if (b'@'..=b'_').contains(&upper) {
                bytes.push(upper & 0x1f);
            }
        }
        if bytes.is_empty() {
            let mut encoded = [0_u8; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
        }
    }
    if meta_state & META_ALT_ON != 0 {
        bytes.insert(0, 0x1b);
    }
    bytes
}

#[derive(Debug)]
struct AndroidTerminalConfiguration;

impl TerminalConfiguration for AndroidTerminalConfiguration {
    fn scrollback_size(&self) -> usize {
        2_000
    }

    fn color_palette(&self) -> ColorPalette {
        android_palette()
    }
}

fn android_palette() -> ColorPalette {
    let mut palette = ColorPalette::default();
    palette.background = SrgbaTuple(0.0, 0.0, 0.0, 1.0);
    palette.foreground = SrgbaTuple(214.0 / 255.0, 224.0 / 255.0, 240.0 / 255.0, 1.0);
    palette
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellSnapshot {
    pub row: usize,
    pub column: usize,
    pub text: String,
    pub width: usize,
    pub style: CellStyleSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellStyleSnapshot {
    pub foreground_rgba: [u8; 4],
    pub background_rgba: [u8; 4],
    pub intensity: u8,
    pub underline: u8,
    pub italic: bool,
    pub strikethrough: bool,
    pub invisible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub columns: usize,
    pub rows: usize,
    pub cursor_column: usize,
    pub cursor_row: usize,
    pub cells: Vec<CellSnapshot>,
    /// Distance in physical rows from the live bottom viewport.
    pub viewport_offset: usize,
    /// Largest valid value for `viewport_offset` in the current scrollback.
    pub max_viewport_offset: usize,
    /// Whether each physical row continues into the following row.
    pub wrapped_rows: Vec<bool>,
}

impl TerminalSnapshot {
    /// Produce a compact representation for the temporary P1 bitmap renderer.
    /// Full grapheme text remains in `cells` for the WezTerm shaping sub-gate.
    pub fn first_codepoints(&self) -> Vec<u32> {
        let mut codepoints = vec![0; self.columns.saturating_mul(self.rows)];
        for cell in &self.cells {
            if cell.row >= self.rows || cell.column >= self.columns {
                continue;
            }
            codepoints[cell.row * self.columns + cell.column] =
                cell.text.chars().next().unwrap_or(' ') as u32;
        }
        codepoints
    }

    /// Return terminal text for an inclusive, row-major cell selection.
    /// Wide-cell continuation columns are mapped back to their grapheme, and
    /// soft-wrapped physical rows are joined without an inserted newline.
    pub fn selected_text(
        &self,
        start_row: usize,
        start_column: usize,
        end_row: usize,
        end_column: usize,
    ) -> String {
        if self.rows == 0 || self.columns == 0 {
            return String::new();
        }
        let start = (
            start_row.min(self.rows - 1),
            start_column.min(self.columns - 1),
        );
        let end = (end_row.min(self.rows - 1), end_column.min(self.columns - 1));
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };

        let mut result = String::new();
        for row in start.0..=end.0 {
            let first_column = if row == start.0 { start.1 } else { 0 };
            let last_column = if row == end.0 {
                end.1
            } else {
                self.columns - 1
            };
            let mut row_text = String::new();
            let mut last_cell_start = None;
            for column in first_column..=last_column {
                let cell = self.cells.iter().find(|cell| {
                    cell.row == row
                        && column >= cell.column
                        && column < cell.column.saturating_add(cell.width.max(1))
                });
                match cell {
                    Some(cell) if last_cell_start != Some(cell.column) => {
                        row_text.push_str(&cell.text);
                        last_cell_start = Some(cell.column);
                    }
                    Some(_) => {}
                    None => {
                        row_text.push(' ');
                        last_cell_start = None;
                    }
                }
            }
            result.push_str(row_text.trim_end_matches(' '));
            if row < end.0 && !self.wrapped_rows.get(row).copied().unwrap_or(false) {
                result.push('\n');
            }
        }
        result
    }

    /// Expand a touched cell to a whitespace-delimited word on that row.
    pub fn word_bounds(&self, row: usize, column: usize) -> (usize, usize) {
        if self.rows == 0 || self.columns == 0 {
            return (0, 0);
        }
        let row = row.min(self.rows - 1);
        let column = column.min(self.columns - 1);
        let occupied = |candidate: usize| {
            self.cells.iter().any(|cell| {
                cell.row == row
                    && candidate >= cell.column
                    && candidate < cell.column.saturating_add(cell.width.max(1))
                    && !cell.text.chars().all(char::is_whitespace)
            })
        };
        if !occupied(column) {
            return (column, column);
        }
        let mut start = column;
        while start > 0 && occupied(start - 1) {
            start -= 1;
        }
        let mut end = column;
        while end + 1 < self.columns && occupied(end + 1) {
            end += 1;
        }
        (start, end)
    }
}

pub struct TerminalModel {
    terminal: Terminal,
}

impl TerminalModel {
    pub fn new(
        columns: usize,
        rows: usize,
        pixel_width: usize,
        pixel_height: usize,
        dpi: u32,
    ) -> Self {
        Self {
            terminal: Terminal::new(
                terminal_size(columns, rows, pixel_width, pixel_height, dpi),
                Arc::new(AndroidTerminalConfiguration),
                "WezTerm Android",
                env!("CARGO_PKG_VERSION"),
                Box::new(Vec::<u8>::new()),
            ),
        }
    }

    pub fn feed(&mut self, bytes: impl AsRef<[u8]>) {
        self.terminal.advance_bytes(bytes);
    }

    pub fn resize(
        &mut self,
        columns: usize,
        rows: usize,
        pixel_width: usize,
        pixel_height: usize,
        dpi: u32,
    ) {
        self.terminal
            .resize(terminal_size(columns, rows, pixel_width, pixel_height, dpi));
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        self.snapshot_with_scrollback_offset(0)
    }

    pub fn snapshot_with_scrollback_offset(&self, requested_offset: usize) -> TerminalSnapshot {
        let size = self.terminal.get_size();
        let cursor = self.terminal.cursor_pos();
        let screen = self.terminal.screen();
        let first_visible_row = screen.phys_row(0);
        let max_viewport_offset = first_visible_row;
        let viewport_offset = requested_offset.min(max_viewport_offset);
        let first_view_row = first_visible_row.saturating_sub(viewport_offset);
        let lines = screen.lines_in_phys_range(first_view_row..first_view_row + size.rows);
        let mut cells = Vec::new();
        let palette = android_palette();
        let default_style = snapshot_attributes(&CellAttributes::blank(), &palette);
        let wrapped_rows = lines
            .iter()
            .map(|line| line.last_cell_was_wrapped())
            .collect();

        for (row, line) in lines.iter().enumerate() {
            for cell in line.visible_cells() {
                let text = cell.str();
                if cell.cell_index() >= size.cols {
                    continue;
                }
                let style = snapshot_attributes(cell.attrs(), &palette);
                if text.trim().is_empty() && style == default_style {
                    continue;
                }
                cells.push(CellSnapshot {
                    row,
                    column: cell.cell_index(),
                    text: text.to_owned(),
                    width: cell.width(),
                    style,
                });
            }
        }

        TerminalSnapshot {
            columns: size.cols,
            rows: size.rows,
            cursor_column: cursor.x.min(size.cols.saturating_sub(1)),
            cursor_row: if viewport_offset == 0 {
                usize::try_from(cursor.y)
                    .unwrap_or(0)
                    .min(size.rows.saturating_sub(1))
            } else {
                // One row beyond the viewport suppresses the renderer cursor.
                size.rows
            },
            cells,
            viewport_offset,
            max_viewport_offset,
            wrapped_rows,
        }
    }
}

fn snapshot_attributes(attrs: &CellAttributes, palette: &ColorPalette) -> CellStyleSnapshot {
    let mut foreground = attrs.foreground();
    let mut background = attrs.background();
    if attrs.intensity() == Intensity::Bold {
        if let ColorAttribute::PaletteIndex(index) = foreground {
            if index < 8 {
                foreground = ColorAttribute::PaletteIndex(index + 8);
            }
        }
    }
    if attrs.reverse() {
        std::mem::swap(&mut foreground, &mut background);
    }

    CellStyleSnapshot {
        foreground_rgba: color_to_rgba(palette.resolve_fg(foreground)),
        background_rgba: color_to_rgba(palette.resolve_bg(background)),
        intensity: attrs.intensity() as u8,
        underline: attrs.underline() as u8,
        italic: attrs.italic(),
        strikethrough: attrs.strikethrough(),
        invisible: attrs.invisible(),
    }
}

fn color_to_rgba(color: SrgbaTuple) -> [u8; 4] {
    let (red, green, blue, alpha) = color.to_srgb_u8();
    [red, green, blue, alpha]
}

fn terminal_size(
    columns: usize,
    rows: usize,
    pixel_width: usize,
    pixel_height: usize,
    dpi: u32,
) -> TerminalSize {
    TerminalSize {
        cols: columns.max(1),
        rows: rows.max(1),
        pixel_width,
        pixel_height,
        dpi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> TerminalModel {
        TerminalModel::new(12, 4, 120, 80, 160)
    }

    #[test]
    fn parses_ansi_and_preserves_wide_cell_columns() {
        let mut model = model();
        model.feed(b"\x1b[2J\x1b[H\x1b[31mA\x1b[0m\xE4\xB8\xADB");

        let snapshot = model.snapshot();
        let occupied: Vec<_> = snapshot
            .cells
            .iter()
            .map(|cell| (cell.column, cell.text.as_str(), cell.width))
            .collect();

        assert_eq!(occupied, vec![(0, "A", 1), (1, "中", 2), (3, "B", 1)]);
        assert_eq!(snapshot.cursor_column, 4);
        assert_eq!(snapshot.cursor_row, 0);
    }

    #[test]
    fn resizes_without_recreating_the_terminal_model() {
        let mut model = model();
        model.feed("persistent");
        model.resize(20, 6, 200, 120, 320);

        let snapshot = model.snapshot();
        assert_eq!((snapshot.columns, snapshot.rows), (20, 6));
        assert!(snapshot.cells.iter().any(|cell| cell.text == "p"));
    }

    #[test]
    fn centers_idle_cat_on_a_pure_black_terminal() {
        let mut model = TerminalModel::new(40, 20, 400, 400, 160);
        model.feed(idle_cat_ansi(40, 20));

        let snapshot = model.snapshot();
        let occupied_rows: Vec<_> = snapshot.cells.iter().map(|cell| cell.row).collect();
        assert_eq!(occupied_rows.iter().min().copied(), Some(5));
        assert_eq!(occupied_rows.iter().max().copied(), Some(13));
        assert!(snapshot.cells.iter().any(|cell| cell.text == "█"));
        assert!(snapshot
            .cells
            .iter()
            .all(|cell| cell.style.background_rgba == [0, 0, 0, 255]));
    }

    #[test]
    fn default_terminal_background_is_pure_black() {
        let mut model = model();
        model.feed("X");
        assert_eq!(
            model.snapshot().cells[0].style.background_rgba,
            [0, 0, 0, 255]
        );
    }

    #[test]
    fn snapshots_terminal_style_attributes() {
        let mut model = model();
        model.feed(b"\x1b[2J\x1b[H\x1b[1;3;4;38;2;1;2;3;48;2;4;5;6mX");

        let snapshot = model.snapshot();
        let style = snapshot
            .cells
            .iter()
            .find(|cell| cell.text == "X")
            .unwrap()
            .style;
        assert_eq!(style.foreground_rgba, [1, 2, 3, 255]);
        assert_eq!(style.background_rgba, [4, 5, 6, 255]);
        assert_eq!(style.intensity, 1);
        assert_eq!(style.underline, 1);
        assert!(style.italic);
        assert!(!style.strikethrough);
        assert!(!style.invisible);
    }

    #[test]
    fn retains_styled_spaces_but_omits_default_blanks() {
        let mut model = model();
        model.feed(b"\x1b[2J\x1b[H \x1b[48;2;7;8;9m \x1b[0m");

        let snapshot = model.snapshot();
        assert_eq!(snapshot.cells.len(), 1);
        assert_eq!(snapshot.cells[0].column, 1);
        assert_eq!(snapshot.cells[0].text, " ");
        assert_eq!(snapshot.cells[0].style.background_rgba, [7, 8, 9, 255]);
    }

    #[test]
    fn preserves_combining_sequence_as_one_terminal_cell() {
        let mut model = model();
        model.feed("e\u{301}");

        let snapshot = model.snapshot();
        assert_eq!(snapshot.cells.len(), 1);
        assert_eq!(snapshot.cells[0].text, "e\u{301}");
        assert_eq!(snapshot.cells[0].width, 1);
        assert_eq!(snapshot.cursor_column, 1);
    }

    #[test]
    fn snapshots_scrollback_at_a_clamped_offset() {
        let mut model = model();
        for index in 0..10 {
            model.feed(format!("line-{index}\r\n"));
        }

        let bottom = model.snapshot();
        let oldest = model.snapshot_with_scrollback_offset(usize::MAX);
        assert!(bottom.max_viewport_offset > 0);
        assert_eq!(oldest.viewport_offset, oldest.max_viewport_offset);
        assert_ne!(bottom.cells, oldest.cells);
        assert_eq!(oldest.cursor_row, oldest.rows);
    }

    #[test]
    fn extracts_word_and_multiline_selection_text() {
        let mut model = model();
        model.feed("alpha beta\r\nA中B");
        let snapshot = model.snapshot();

        assert_eq!(snapshot.word_bounds(0, 7), (6, 9));
        assert_eq!(snapshot.selected_text(0, 0, 0, 4), "alpha");
        assert_eq!(snapshot.selected_text(0, 6, 1, 3), "beta\nA中B");
    }

    #[test]
    fn encodes_printable_ctrl_and_alt_input() {
        assert_eq!(android_key_bytes(0, 'a' as i32, 0), b"a");
        assert_eq!(android_key_bytes(0, '中' as i32, 0), "中".as_bytes());
        assert_eq!(android_key_bytes(0, 'c' as i32, 0x1000), b"\x03");
        assert_eq!(android_key_bytes(0, 'x' as i32, 0x02), b"\x1bx");
    }

    #[test]
    fn encodes_navigation_editing_and_alt_special_keys() {
        assert_eq!(android_key_bytes(19, 0, 0), b"\x1b[A");
        assert_eq!(android_key_bytes(122, 0, 0), b"\x1b[H");
        assert_eq!(android_key_bytes(123, 0, 0), b"\x1b[F");
        assert_eq!(android_key_bytes(92, 0, 0), b"\x1b[5~");
        assert_eq!(android_key_bytes(112, 0, 0), b"\x1b[3~");
        assert_eq!(android_key_bytes(21, 0, 0x02), b"\x1b\x1b[D");
    }

    #[test]
    fn encodes_all_function_keys() {
        let expected: [&[u8]; 12] = [
            b"\x1bOP",
            b"\x1bOQ",
            b"\x1bOR",
            b"\x1bOS",
            b"\x1b[15~",
            b"\x1b[17~",
            b"\x1b[18~",
            b"\x1b[19~",
            b"\x1b[20~",
            b"\x1b[21~",
            b"\x1b[23~",
            b"\x1b[24~",
        ];
        for (offset, expected) in expected.into_iter().enumerate() {
            assert_eq!(android_key_bytes(131 + offset as i32, 0, 0), expected);
        }
    }
}
