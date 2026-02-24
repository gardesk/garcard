use anyhow::{Context, Result};
use gartk_core::{Color, InputEvent, Key, KeyEvent, Rect, Theme};
use gartk_render::{Renderer, TextStyle, copy_surface_to_window};
use gartk_x11::{
    Connection, EventLoop, EventLoopConfig, Window, WindowConfig, monitor_at_pointer,
    primary_monitor,
};
use std::time::{Duration, Instant};
use x11rb::protocol::xproto::ConnectionExt;

const DIALOG_WIDTH: u32 = 560;
const DIALOG_HEIGHT: u32 = 240;
const CARD_PADDING: i32 = 18;
const ERROR_BLINK_INTERVAL: Duration = Duration::from_millis(180);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    Secret,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptTone {
    Default,
    Success,
    Error,
}

#[derive(Debug, Clone)]
pub struct PromptRequest {
    pub message: String,
    pub mode: PromptMode,
    pub timeout_secs: u64,
    pub tone: PromptTone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptExit {
    Submitted(String),
    Canceled,
    TimedOut,
}

struct PromptDialog {
    window: Window,
    renderer: Renderer,
    gc: u32,
    request: PromptRequest,
    input: String,
    cursor: usize,
    exit: Option<PromptExit>,
    deadline: Option<Instant>,
    remaining_secs: Option<u64>,
    backdrop: Color,
    card_background: Color,
    card_border: Color,
    accent: Color,
    success: Color,
    error: Color,
    error_blink_on: bool,
    last_blink_toggle: Instant,
}

pub fn run_prompt_dialog(request: PromptRequest) -> Result<PromptExit> {
    let conn = Connection::connect(None).context("failed to connect to X11 display")?;
    let (x, y) = centered_position(&conn, DIALOG_WIDTH, DIALOG_HEIGHT);
    let window = Window::create(
        conn.clone(),
        WindowConfig::dialog()
            .title("garcard authentication")
            .class("garcard")
            .position(x, y)
            .size(DIALOG_WIDTH, DIALOG_HEIGHT)
            .transparent(true)
            .modal(true),
    )
    .context("failed to create prompt window")?;
    window.focus().context("failed to focus prompt window")?;

    let mut dialog = PromptDialog::new(window, request)?;
    dialog.run()
}

fn centered_position(conn: &Connection, width: u32, height: u32) -> (i32, i32) {
    let monitor = monitor_at_pointer(conn)
        .or_else(|_| primary_monitor(conn))
        .ok();
    if let Some(monitor) = monitor {
        let x = monitor.rect.x + (monitor.rect.width as i32 - width as i32) / 2;
        let y = monitor.rect.y + (monitor.rect.height as i32 - height as i32) / 3;
        return (x, y);
    }

    (
        (conn.screen_width() as i32 - width as i32) / 2,
        (conn.screen_height() as i32 - height as i32) / 3,
    )
}

impl PromptDialog {
    fn new(window: Window, request: PromptRequest) -> Result<Self> {
        let mut theme = Theme::dark();
        theme.font_family = "Sans".to_string();
        theme.font_size = 14.0;
        let backdrop = Color::from_hex("#0a0b10").context("invalid prompt backdrop color")?;
        let card_background =
            Color::from_hex("#111318").context("invalid prompt card background color")?;
        let card_border = Color::from_hex("#2c3442").context("invalid prompt card border color")?;
        let accent = Color::from_hex("#8ab4f8").context("invalid prompt accent color")?;
        let success = Color::from_hex("#41c87a").context("invalid prompt success color")?;
        let error = Color::from_hex("#ff5f6d").context("invalid prompt error color")?;
        let size = window.size();
        let renderer = Renderer::with_theme(size.width, size.height, theme)?;

        let conn = window.connection();
        let gc = conn.generate_id()?;
        conn.inner()
            .create_gc(gc, window.id(), &Default::default())?;
        conn.flush()?;

        let deadline = if request.timeout_secs > 0 {
            Some(Instant::now() + Duration::from_secs(request.timeout_secs))
        } else {
            None
        };

        Ok(Self {
            window,
            renderer,
            gc,
            request,
            input: String::new(),
            cursor: 0,
            exit: None,
            deadline,
            remaining_secs: None,
            backdrop,
            card_background,
            card_border,
            accent,
            success,
            error,
            error_blink_on: true,
            last_blink_toggle: Instant::now(),
        })
    }

    fn run(&mut self) -> Result<PromptExit> {
        let mut event_loop = EventLoop::new(&self.window, EventLoopConfig::default())?;
        self.refresh_timeout();
        self.render()?;

        event_loop.run(|ev, event| {
            match event {
                InputEvent::Key(key_event) if key_event.pressed => {
                    self.handle_key(&key_event);
                    ev.request_redraw();
                }
                InputEvent::Resize { width, height } => {
                    if let Err(err) = self.renderer.resize(width, height) {
                        tracing::error!(error = %err, "failed to resize prompt renderer");
                        self.exit = Some(PromptExit::Canceled);
                    }
                    ev.request_redraw();
                }
                InputEvent::Expose => {
                    ev.request_redraw();
                }
                InputEvent::CloseRequested => {
                    self.exit = Some(PromptExit::Canceled);
                }
                InputEvent::Idle => {
                    if self.refresh_timeout() {
                        ev.request_redraw();
                    }
                }
                _ => {}
            }

            if ev.needs_redraw() {
                let _ = self.render();
                ev.redraw_done();
            }

            Ok(self.exit.is_none())
        })?;

        Ok(self.exit.take().unwrap_or(PromptExit::Canceled))
    }

    fn refresh_timeout(&mut self) -> bool {
        let mut changed = false;

        let Some(deadline) = self.deadline else {
            if self.request.tone == PromptTone::Error
                && Instant::now().duration_since(self.last_blink_toggle) >= ERROR_BLINK_INTERVAL
            {
                self.last_blink_toggle = Instant::now();
                self.error_blink_on = !self.error_blink_on;
                changed = true;
            }
            return changed;
        };

        let now = Instant::now();
        if now >= deadline {
            self.exit = Some(PromptExit::TimedOut);
            return true;
        }

        let remaining = deadline.duration_since(now).as_secs();
        if self.remaining_secs != Some(remaining) {
            self.remaining_secs = Some(remaining);
            changed = true;
        }

        if self.request.tone == PromptTone::Error
            && now.duration_since(self.last_blink_toggle) >= ERROR_BLINK_INTERVAL
        {
            self.last_blink_toggle = now;
            self.error_blink_on = !self.error_blink_on;
            changed = true;
        }

        changed
    }

    fn handle_key(&mut self, key_event: &KeyEvent) {
        if self.request.tone != PromptTone::Default {
            match key_event.key {
                Key::Escape | Key::Return => {
                    self.exit = Some(PromptExit::Canceled);
                }
                _ => {}
            }
            return;
        }

        match key_event.key {
            Key::Escape => {
                self.exit = Some(PromptExit::Canceled);
            }
            Key::Return => {
                let submitted = std::mem::take(&mut self.input);
                self.cursor = 0;
                self.exit = Some(PromptExit::Submitted(submitted));
            }
            Key::Left => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
            }
            Key::Right => {
                if self.cursor < self.input.chars().count() {
                    self.cursor += 1;
                }
            }
            Key::Home => {
                self.cursor = 0;
            }
            Key::End => {
                self.cursor = self.input.chars().count();
            }
            Key::Backspace => {
                remove_char_before(&mut self.input, &mut self.cursor);
            }
            Key::Delete => {
                remove_char_at(&mut self.input, self.cursor);
            }
            Key::Space => {
                if key_event.modifiers.is_empty() || key_event.modifiers.shift {
                    insert_char_at(&mut self.input, self.cursor, ' ');
                    self.cursor += 1;
                }
            }
            Key::Char(ch) => {
                if ch.is_control() {
                    return;
                }
                if key_event.modifiers.ctrl
                    || key_event.modifiers.alt
                    || key_event.modifiers.super_key
                {
                    return;
                }
                insert_char_at(&mut self.input, self.cursor, ch);
                self.cursor += 1;
            }
            _ => {}
        }
    }

    fn render(&mut self) -> Result<()> {
        let size = self.renderer.size();
        let width = size.width as i32;
        let height = size.height as i32;
        let theme = self.renderer.theme().clone();
        let accent = match self.request.tone {
            PromptTone::Default => self.accent,
            PromptTone::Success => self.success,
            PromptTone::Error => {
                if self.error_blink_on {
                    self.error
                } else {
                    self.card_border
                }
            }
        };

        self.renderer.clear_color(self.backdrop)?;

        let card_rect = Rect::new(
            CARD_PADDING,
            CARD_PADDING,
            (width - CARD_PADDING * 2) as u32,
            (height - CARD_PADDING * 2) as u32,
        );
        self.renderer
            .fill_rounded_rect(card_rect, 12.0, self.card_background)?;
        self.renderer
            .stroke_rounded_rect(card_rect, 12.0, self.card_border, 2.0)?;

        let title_style = TextStyle::new()
            .font_family(theme.font_family.clone())
            .font_size(18.0)
            .color(theme.foreground);
        self.renderer.text(
            "Authentication Required",
            (CARD_PADDING + 16) as f64,
            (CARD_PADDING + 14) as f64,
            &title_style,
        )?;

        let message_style = TextStyle::new()
            .font_family(theme.font_family.clone())
            .font_size(13.0)
            .color(theme.item_description)
            .max_width(card_rect.width as i32 - 32)
            .wrap(true)
            .ellipsize(true);
        self.renderer.text(
            &self.request.message,
            (CARD_PADDING + 16) as f64,
            (CARD_PADDING + 46) as f64,
            &message_style,
        )?;

        let input_label = match self.request.mode {
            PromptMode::Secret => "Password",
            PromptMode::Plain => "Response",
        };
        let label_style = TextStyle::new()
            .font_family(theme.font_family.clone())
            .font_size(12.0)
            .color(theme.input_placeholder);
        self.renderer.text(
            input_label,
            (CARD_PADDING + 16) as f64,
            (CARD_PADDING + 108) as f64,
            &label_style,
        )?;

        let input_rect = Rect::new(
            CARD_PADDING + 16,
            CARD_PADDING + 126,
            (width - (CARD_PADDING + 16) * 2) as u32,
            40,
        );
        self.renderer
            .fill_rounded_rect(input_rect, 8.0, theme.input_background)?;
        self.renderer
            .stroke_rounded_rect(input_rect, 8.0, accent.with_alpha(0.7), 1.5)?;

        let display_input = display_value(&self.input, self.request.mode);
        let input_style = TextStyle::new()
            .font_family(theme.font_family.clone())
            .font_size(14.0)
            .color(theme.input_foreground);
        let input_y = input_rect.y + 12;
        let input_x = input_rect.x + 10;
        self.renderer
            .text(&display_input, input_x as f64, input_y as f64, &input_style)?;

        let cursor_prefix = display_prefix(&self.input, self.cursor, self.request.mode);
        if self.request.tone == PromptTone::Default {
            let cursor_size = self.renderer.measure_text(&cursor_prefix, &input_style)?;
            let cursor_x = input_x + cursor_size.width as i32;
            self.renderer.fill_rect(
                Rect::new(cursor_x, input_rect.y + 8, 2, input_rect.height - 16),
                accent,
            )?;
        }

        let footer_style = TextStyle::new()
            .font_family(theme.font_family)
            .font_size(12.0)
            .color(theme.item_description);
        let footer_text = if self.request.tone == PromptTone::Default {
            "Enter submit   Esc cancel"
        } else {
            "Esc dismiss"
        };
        self.renderer.text(
            footer_text,
            (CARD_PADDING + 16) as f64,
            (height - CARD_PADDING - 26) as f64,
            &footer_style,
        )?;

        if let Some(remaining) = self.remaining_secs {
            let timer_text = format!("timeout {}s", remaining);
            let timer_style = TextStyle::new()
                .font_family("Sans")
                .font_size(12.0)
                .color(accent);
            let timer_size = self.renderer.measure_text(&timer_text, &timer_style)?;
            self.renderer.text(
                &timer_text,
                (width - CARD_PADDING - timer_size.width as i32 - 16) as f64,
                (height - CARD_PADDING - 26) as f64,
                &timer_style,
            )?;
        }

        self.renderer.flush();
        copy_surface_to_window(self.renderer.surface_mut(), &self.window, self.gc, 0, 0)?;
        Ok(())
    }
}

impl Drop for PromptDialog {
    fn drop(&mut self) {
        scrub_string(&mut self.input);
        let _ = self.window.connection().inner().free_gc(self.gc);
    }
}

fn display_value(input: &str, mode: PromptMode) -> String {
    match mode {
        PromptMode::Secret => "*".repeat(input.chars().count()),
        PromptMode::Plain => input.to_string(),
    }
}

fn display_prefix(input: &str, cursor: usize, mode: PromptMode) -> String {
    let prefix = prefix_chars(input, cursor);
    match mode {
        PromptMode::Secret => "*".repeat(prefix.chars().count()),
        PromptMode::Plain => prefix,
    }
}

fn prefix_chars(input: &str, char_count: usize) -> String {
    input.chars().take(char_count).collect()
}

fn char_to_byte_index(input: &str, char_index: usize) -> usize {
    input
        .char_indices()
        .nth(char_index)
        .map(|(byte, _)| byte)
        .unwrap_or_else(|| input.len())
}

fn insert_char_at(input: &mut String, char_index: usize, value: char) {
    let byte_index = char_to_byte_index(input, char_index);
    input.insert(byte_index, value);
}

fn remove_char_before(input: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }

    let end = char_to_byte_index(input, *cursor);
    let start = char_to_byte_index(input, *cursor - 1);
    input.drain(start..end);
    *cursor -= 1;
}

fn remove_char_at(input: &mut String, cursor: usize) {
    if cursor >= input.chars().count() {
        return;
    }

    let start = char_to_byte_index(input, cursor);
    let end = char_to_byte_index(input, cursor + 1);
    input.drain(start..end);
}

fn scrub_string(value: &mut String) {
    if value.is_empty() {
        return;
    }
    let mut bytes = std::mem::take(value).into_bytes();
    bytes.fill(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_value_masks_secret_text() {
        assert_eq!(display_value("secret", PromptMode::Secret), "******");
        assert_eq!(display_value("plain", PromptMode::Plain), "plain");
    }

    #[test]
    fn insert_and_remove_respect_char_positions() {
        let mut value = String::from("abcd");
        insert_char_at(&mut value, 2, 'X');
        assert_eq!(value, "abXcd");

        let mut cursor = 3;
        remove_char_before(&mut value, &mut cursor);
        assert_eq!(value, "abcd");
        assert_eq!(cursor, 2);

        remove_char_at(&mut value, 1);
        assert_eq!(value, "acd");
    }

    #[test]
    fn display_prefix_uses_cursor_and_mode() {
        assert_eq!(display_prefix("hello", 2, PromptMode::Plain), "he");
        assert_eq!(display_prefix("hello", 2, PromptMode::Secret), "**");
    }

    #[test]
    fn scrub_string_clears_input() {
        let mut value = "top-secret".to_string();
        scrub_string(&mut value);
        assert!(value.is_empty());
    }
}
