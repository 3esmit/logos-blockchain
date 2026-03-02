//! Logging utilities for TUI applications running in raw terminal mode.

use ratatui::crossterm::terminal;

/// A writer that confines log output to the bottom half of the terminal.
///
/// Before each write it:
/// 1. Sets the DECSTBM scroll region to the bottom half, so that any scroll
///    caused by new lines never displaces the top half.
/// 2. Moves the cursor to the last row of that region, so the newest line
///    always appears at the bottom and older lines scroll upward within the
///    region.
/// 3. Converts bare `\n` to `\r\n` for correct rendering in raw mode.
#[derive(Clone, Copy)]
pub struct RawModeWriter;

impl std::io::Write for RawModeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut out = std::io::stdout().lock();

        let (_, height) = terminal::size().unwrap_or((80, 24));
        let top_row = height / 2 + 1; // 1-based: first row of bottom half
        let bottom_row = height; // 1-based: last row of terminal

        // Set scroll region to the bottom half, then move cursor to its last row.
        // Any subsequent \n will scroll only within [top_row, bottom_row].
        write!(out, "\x1b[{top_row};{bottom_row}r\x1b[{bottom_row};1H")?;

        let mut start = 0;
        for i in 0..buf.len() {
            if buf[i] == b'\n' && (i == 0 || buf[i - 1] != b'\r') {
                out.write_all(&buf[start..i])?;
                out.write_all(b"\r\n")?;
                start = i + 1;
            }
        }
        out.write_all(&buf[start..])?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RawModeWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        *self
    }
}
