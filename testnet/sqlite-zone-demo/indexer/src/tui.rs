#![allow(clippy::allow_attributes_without_reason)]

//! The bulk of the actual user interface logic.

use std::{
    fmt::{self, Debug, Formatter},
    ops::{ControlFlow, Deref, DerefMut},
    time::Duration,
};

use arboard::Clipboard;
use ratatui::{
    Frame,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind},
    layout::{Constraint, Margin, Rect},
    style::Modifier,
    widgets::{
        Clear, Paragraph, Row, Table, TableState,
        block::{Block, BorderType},
    },
};
use tui_textarea::TextArea;
use zeroize::Zeroizing;

use crate::{
    config::Theme,
    crypto::DecryptionInput,
    db::{DatabaseReadOnly, DisplayItem},
    error::{Error, Result},
};

/// The top-level UI state, the basis of rendering.
#[derive(Debug)]
pub struct State {
    db: DatabaseReadOnly,
    clipboard: ClipboardDebugWrapper,
    theme: Theme,
    is_running: bool,
    passwd_entry: Option<PasswordEntryState>,
    find: Option<FindItemState>,
    popup_error: Option<Error>,
    items: Vec<DisplayItem>,
    table_state: TableState,
}

impl State {
    pub fn new(db: DatabaseReadOnly, theme: Theme) -> Result<Self> {
        let items = db.list_items_for_display(None)?;
        let clipboard = ClipboardDebugWrapper(Clipboard::new()?);

        let table_state =
            TableState::new().with_selected(if items.is_empty() { None } else { Some(0) });

        Ok(Self {
            db,
            clipboard,
            theme,
            is_running: true,
            passwd_entry: None,
            find: None,
            popup_error: None,
            items,
            table_state,
        })
    }

    /// Returns `true` as long as the application should run.
    /// Once this returns `false`, the application will exit.
    pub const fn is_running(&self) -> bool {
        self.is_running
    }

    /// Top-level widget rendering.
    pub fn draw(&mut self, frame: &mut Frame) {
        let half_screen = {
            let full = frame.area();
            Rect {
                height: full.height / 2,
                ..full
            }
        };
        let bottom_input_height = 3;
        let mut table_area = {
            let mut area = half_screen;
            area.height -= bottom_input_height;
            area
        };
        let bottom_input_area = Rect {
            x: table_area.x,
            y: table_area.y + table_area.height,
            width: table_area.width,
            height: bottom_input_height,
        };
        let table = self.main_table();

        if let Some(passwd_entry) = self.passwd_entry.as_mut() {
            frame.render_widget(&passwd_entry.enc_pass, bottom_input_area);
        } else if let Some(find_state) = self.find.as_mut() {
            frame.render_widget(&find_state.search_term, bottom_input_area);
        } else {
            table_area = half_screen;
        }

        frame.render_stateful_widget(table, table_area, &mut self.table_state);

        if let Some(error) = self.popup_error.as_ref() {
            let margin = Margin {
                horizontal: half_screen.width.saturating_sub(72 + 2) / 2,
                vertical: half_screen.height.saturating_sub(3 + 2) / 2,
            };
            let dialog_area = half_screen.inner(margin);
            let modal = self.error_modal(error);

            frame.render_widget(Clear, dialog_area);
            frame.render_widget(modal, dialog_area);
        }
    }

    fn main_table(&self) -> Table<'static> {
        Table::new(
            self.items.iter().map(|item| {
                Row::new([
                    item.label.clone(),
                    item.account.clone().unwrap_or_default(),
                    item.last_modified_at.format("%F %T").to_string(),
                ])
            }),
            [
                Constraint::Percentage(40),
                Constraint::Percentage(40),
                Constraint::Min(24),
            ],
        )
        .header(
            Row::new(["Title", "Username or account", "Modified at (UTC)"])
                .style(self.theme.default().add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(Modifier::REVERSED)
        .block(
            Block::bordered()
                .title(format!(
                    " SteelSafe v{} (read-only) ",
                    env!("CARGO_PKG_VERSION")
                ))
                .title_bottom(" [C]opy secret ")
                .title_bottom(" [F]ind ")
                .title_bottom(" [1] First ")
                .title_bottom(" [0] Last ")
                .title_bottom(" [R]efresh ")
                .title_bottom(" [Q]uit ")
                .border_type(BorderType::Rounded)
                .border_style(if self.main_table_has_focus() {
                    self.theme.border().add_modifier(Modifier::BOLD)
                } else {
                    self.theme.border()
                }),
        )
        .style(self.theme.default())
    }

    fn error_modal(&self, error: &Error) -> Paragraph<'static> {
        let block = Block::bordered()
            .title(" Error ")
            .title_bottom(" <Esc> Close ")
            .border_type(BorderType::Rounded)
            .border_style(self.theme.error().add_modifier(Modifier::BOLD));

        Paragraph::new(format!("\n{error}\n"))
            .centered()
            .block(block)
            .style(self.theme.error())
    }

    /// Event polling and error handling.
    pub fn handle_events(&mut self) {
        if let Err(error) = self.handle_events_impl() {
            self.popup_error = Some(error);
        }
    }

    /// The bulk of the actual event handling logic.
    fn handle_events_impl(&mut self) -> Result<()> {
        if !event::poll(Duration::from_millis(50))? {
            return Ok(());
        }
        let event = event::read()?;

        let event = match self.handle_error_input(event)? {
            ControlFlow::Break(()) => return Ok(()),
            ControlFlow::Continue(event) => event,
        };
        let event = match self.handle_passwd_entry_input(event)? {
            ControlFlow::Break(()) => return Ok(()),
            ControlFlow::Continue(event) => event,
        };
        let event = match self.handle_find_input(event)? {
            ControlFlow::Break(()) => return Ok(()),
            ControlFlow::Continue(event) => event,
        };

        self.handle_main_table_event(&event)
    }

    /// Handles events when the main table has focus.
    fn handle_main_table_event(&mut self, event: &Event) -> Result<()> {
        if let Event::Mouse(mouse) = event {
            match mouse.kind {
                MouseEventKind::ScrollDown => {
                    self.table_state.select_next();
                }
                MouseEventKind::ScrollUp => {
                    self.table_state.select_previous();
                }
                _ => {}
            }
            return Ok(());
        }

        let Event::Key(key) = event else {
            return Ok(());
        };

        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k' | 'K') => {
                self.table_state.select_previous();
            }
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j' | 'J') => {
                self.table_state.select_next();
            }
            KeyCode::Char('1') => {
                self.table_state.select_first();
            }
            KeyCode::Char('0') => {
                self.table_state.select_last();
            }
            KeyCode::Char('r' | 'R') => {
                self.sync_data(true)?;
            }
            KeyCode::Char('c' | 'C') | KeyCode::Enter => {
                self.passwd_entry = Some(PasswordEntryState::with_theme(self.theme.clone()));
            }
            KeyCode::Char('f' | 'F' | '/') => {
                // if we are already in find mode, do NOT reset
                // the search term, just give back focus.
                if let Some(find_state) = self.find.as_mut() {
                    find_state.set_focus(true);
                } else {
                    self.find = Some(FindItemState::with_theme(self.theme.clone()));
                }
            }
            KeyCode::Char('q' | 'Q') => {
                self.is_running = false;
            }
            _ => {}
        }

        Ok(())
    }

    /// Handles events when the error modal is open.
    #[expect(clippy::unnecessary_wraps)]
    fn handle_error_input(&mut self, event: Event) -> Result<ControlFlow<(), Event>> {
        if self.popup_error.is_none() {
            return Ok(ControlFlow::Continue(event));
        }

        if let Event::Key(evt) = event
            && evt.code == KeyCode::Esc
        {
            self.popup_error = None;
        }

        Ok(ControlFlow::Break(()))
    }

    /// Handles events for the password entry panel before decrypting a secret.
    fn handle_passwd_entry_input(&mut self, event: Event) -> Result<ControlFlow<(), Event>> {
        let Some(passwd_entry) = self.passwd_entry.as_mut() else {
            return Ok(ControlFlow::Continue(event));
        };

        match event {
            Event::Key(evt) => match evt.code {
                KeyCode::Esc => {
                    self.passwd_entry = None;
                }
                KeyCode::Enter => {
                    let password = Zeroizing::new(passwd_entry.enc_pass.lines().join("\n"));
                    self.passwd_entry = None;
                    self.copy_secret_to_clipboard(&password)?;
                }
                KeyCode::Char('h' | 'H') if evt.modifiers.contains(KeyModifiers::CONTROL) => {
                    passwd_entry.toggle_show_enc_pass();
                }
                _ => {
                    passwd_entry.enc_pass.input(event);
                }
            },
            _ => {
                passwd_entry.enc_pass.input(event);
            }
        }

        Ok(ControlFlow::Break(()))
    }

    /// Handles events for the Find panel.
    fn handle_find_input(&mut self, event: Event) -> Result<ControlFlow<(), Event>> {
        let Some(find_state) = self.find.as_mut() else {
            return Ok(ControlFlow::Continue(event));
        };

        match event {
            Event::Key(evt) => match evt.code {
                KeyCode::Esc => {
                    self.find = None;
                    self.sync_data(true)?;
                    Ok(ControlFlow::Break(()))
                }
                KeyCode::Enter if find_state.has_focus => {
                    find_state.set_focus(false);
                    Ok(ControlFlow::Break(()))
                }
                _ if find_state.has_focus => {
                    find_state.search_term.input(event);
                    self.sync_data(true)?;
                    Ok(ControlFlow::Break(()))
                }
                _ => Ok(ControlFlow::Continue(event)),
            },
            _ => Ok(ControlFlow::Continue(event)),
        }
    }

    /// Reloads the contents of the database from disk to memory.
    /// If `adjust_selection` is set, the last item of the table
    /// will be selected. This is useful after certain operations
    /// that act destructively on the table state (e.g., search).
    fn sync_data(&mut self, adjust_selection: bool) -> Result<()> {
        let search_term = self.find.as_ref().and_then(|find_state| {
            find_state
                .search_term
                .lines()
                .first()
                .map(|line| format!("%{}%", line.trim()))
        });
        self.items = self.db.list_items_for_display(search_term.as_deref())?;

        #[expect(unused_parens)]
        if (adjust_selection
            && !self.items.is_empty()
            && self
                .table_state
                .selected()
                .is_none_or(|idx| idx >= self.items.len()))
        {
            self.table_state.select_last();
        }

        Ok(())
    }

    /// Actually copy the decrypted plaintext secret to the clipboard.
    /// We can't zeroize the clipboard content, so we don't even bother.
    fn copy_secret_to_clipboard(&mut self, enc_pass: &str) -> Result<()> {
        let index = self
            .table_state
            .selected()
            .ok_or(Error::SelectionRequired)?;
        let uid = self.items[index].uid;
        let item = self.db.item_by_id(uid)?;

        let input = DecryptionInput {
            encrypted_secret: &item.encrypted_secret,
            kdf_salt: item.kdf_salt,
            auth_nonce: item.auth_nonce,
            label: item.label.as_str(),
            account: item.account.as_deref(),
            last_modified_at: item.last_modified_at,
        };
        let plaintext_secret = input.decrypt_and_verify(enc_pass.as_bytes())?;

        // we do NOT use `String::from_utf8()`, because that would copy the
        // bytes, and complicate correct zeroization of the secret on error.
        let secret_str = std::str::from_utf8(&plaintext_secret)?;

        self.clipboard.set_text(secret_str).map_err(Into::into)
    }

    /// The main table has focus when none of the other widgets do.
    fn main_table_has_focus(&self) -> bool {
        (self.find.is_none() || self.find.as_ref().is_some_and(|find| !find.has_focus))
            && self.passwd_entry.is_none()
            && self.popup_error.is_none()
    }
}

#[derive(Debug)]
struct PasswordEntryState {
    is_visible: bool,
    enc_pass: TextArea<'static>,
    theme: Theme,
}

impl PasswordEntryState {
    fn with_theme(theme: Theme) -> Self {
        let mut enc_pass = TextArea::default();
        enc_pass.set_style(theme.default());

        // set up text field style
        let mut state = Self {
            is_visible: false,
            enc_pass,
            theme,
        };
        state.set_visible(false);
        state
    }

    fn toggle_show_enc_pass(&mut self) {
        self.set_visible(!self.is_visible);
    }

    fn set_visible(&mut self, is_visible: bool) {
        self.is_visible = is_visible;

        if self.is_visible {
            self.enc_pass.clear_mask_char();
        } else {
            self.enc_pass.set_mask_char('\u{25cf}');
        }

        let show_hide_title = format!(
            " <^H> {} password ",
            if self.is_visible { "Hide" } else { "Show" },
        );

        self.enc_pass.set_block(
            Block::bordered()
                .title(" Enter decryption (master) password ")
                .title_bottom(" <Enter> OK ")
                .title_bottom(" <Esc> Cancel ")
                .title_bottom(show_hide_title)
                .border_type(BorderType::Rounded)
                .border_style(self.theme.border().add_modifier(Modifier::BOLD)),
        );
    }
}

#[derive(Debug)]
struct FindItemState {
    search_term: TextArea<'static>,
    has_focus: bool,
    theme: Theme,
}

impl FindItemState {
    fn with_theme(theme: Theme) -> Self {
        let mut search_term = TextArea::default();

        search_term.set_block(
            Block::bordered()
                .title(" Search term ")
                .title_bottom(" <Enter> Focus secrets ")
                .title_bottom(" <Esc> Exit search ")
                .border_type(BorderType::Rounded),
        );

        let mut state = Self {
            search_term,
            has_focus: true,
            theme,
        };
        state.set_focus(true);
        state
    }

    fn set_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;

        let block = self.search_term.block().cloned().unwrap_or_default();

        if self.has_focus {
            self.search_term
                .set_style(self.theme.default().add_modifier(Modifier::BOLD));
            self.search_term
                .set_block(block.border_style(self.theme.border().add_modifier(Modifier::BOLD)));
        } else {
            self.search_term.set_style(self.theme.default());
            self.search_term
                .set_block(block.border_style(self.theme.border()));
        }
    }
}

/// The sole purpose of this is to implement `Debug` so that it doesn't break
/// literally everything.
struct ClipboardDebugWrapper(Clipboard);

impl Debug for ClipboardDebugWrapper {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Clipboard").finish_non_exhaustive()
    }
}

impl Deref for ClipboardDebugWrapper {
    type Target = Clipboard;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ClipboardDebugWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
