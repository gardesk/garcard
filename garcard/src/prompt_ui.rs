use anyhow::{Context, Result};
use gartk_core::{Color, InputEvent, Key, KeyEvent, Rect, Theme};
use gartk_render::{Renderer, TextStyle, copy_surface_to_window};
use gartk_x11::{
    Connection, EventLoop, EventLoopConfig, Window, WindowConfig, monitor_at_pointer,
    primary_monitor,
};
use std::time::{Duration, Instant};
use x11rb::connection::Connection as X11Connection;
use x11rb::protocol::xproto::ConnectionExt;

const BASE_DIALOG_WIDTH: u32 = 560;
const BASE_DIALOG_HEIGHT: u32 = 240;
const BASE_CARD_PADDING: i32 = 18;
const ERROR_BLINK_INTERVAL: Duration = Duration::from_millis(180);
const RETRY_ERROR_FLASH_DURATION: Duration = Duration::from_millis(900);
const DEFAULT_UI_SCALE: f32 = 1.0;
const MIN_UI_SCALE: f32 = 0.8;
const MAX_UI_SCALE: f32 = 2.0;

#[derive(Debug, Clone, Copy)]
struct PromptStrings {
    title_auth_required: &'static str,
    label_password: &'static str,
    label_response: &'static str,
    footer_wait: &'static str,
    footer_controls: &'static str,
    timeout_label: &'static str,
}

impl PromptStrings {
    fn for_locale(locale: &str) -> Self {
        match locale_bucket(locale) {
            "es" => Self {
                title_auth_required: "Autenticacion requerida",
                label_password: "Contrasena",
                label_response: "Respuesta",
                footer_wait: "Espere",
                footer_controls: "Enter enviar   Esc cancelar",
                timeout_label: "tiempo",
            },
            _ => Self {
                title_auth_required: "Authentication Required",
                label_password: "Password",
                label_response: "Response",
                footer_wait: "Please wait",
                footer_controls: "Enter submit   Esc cancel",
                timeout_label: "timeout",
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PromptPalette {
    backdrop: Color,
    card_background: Color,
    card_border: Color,
    accent: Color,
    success: Color,
    error: Color,
    focus_ring: Color,
}

impl PromptPalette {
    fn from_high_contrast(high_contrast: bool) -> Result<Self> {
        if high_contrast {
            return Ok(Self {
                backdrop: Color::from_hex("#000000")
                    .context("invalid high-contrast prompt backdrop color")?,
                card_background: Color::from_hex("#050505")
                    .context("invalid high-contrast prompt card background color")?,
                card_border: Color::from_hex("#f2f2f2")
                    .context("invalid high-contrast prompt card border color")?,
                accent: Color::from_hex("#30d5ff")
                    .context("invalid high-contrast prompt accent color")?,
                success: Color::from_hex("#54f07a")
                    .context("invalid high-contrast prompt success color")?,
                error: Color::from_hex("#ff5f6d")
                    .context("invalid high-contrast prompt error color")?,
                focus_ring: Color::from_hex("#ffffff")
                    .context("invalid high-contrast prompt focus ring color")?,
            });
        }

        Ok(Self {
            backdrop: Color::from_hex("#0a0b10").context("invalid prompt backdrop color")?,
            card_background: Color::from_hex("#111318")
                .context("invalid prompt card background color")?,
            card_border: Color::from_hex("#2c3442").context("invalid prompt card border color")?,
            accent: Color::from_hex("#8ab4f8").context("invalid prompt accent color")?,
            success: Color::from_hex("#41c87a").context("invalid prompt success color")?,
            error: Color::from_hex("#ff5f6d").context("invalid prompt error color")?,
            focus_ring: Color::from_hex("#d5e3ff").context("invalid prompt focus ring color")?,
        })
    }
}

#[derive(Debug, Clone)]
struct PromptUiConfig {
    locale: String,
    ui_scale: f32,
    high_contrast: bool,
}

impl PromptUiConfig {
    fn from_env() -> Self {
        let locale = std::env::var("GARCARD_LOCALE")
            .or_else(|_| std::env::var("LANG"))
            .unwrap_or_else(|_| "en_US.UTF-8".to_string());
        let ui_scale = prompt_scale_from_env(std::env::var("GARCARD_PROMPT_SCALE").ok().as_deref());
        let high_contrast = parse_bool_env(
            std::env::var("GARCARD_PROMPT_HIGH_CONTRAST")
                .ok()
                .as_deref(),
        );
        Self {
            locale,
            ui_scale,
            high_contrast,
        }
    }
}

fn locale_bucket(locale: &str) -> &'static str {
    let normalized = locale.trim().to_ascii_lowercase();
    if normalized.starts_with("es") {
        "es"
    } else {
        "en"
    }
}

fn prompt_scale_from_env(raw: Option<&str>) -> f32 {
    let parsed = raw
        .and_then(|value| value.trim().parse::<f32>().ok())
        .unwrap_or(DEFAULT_UI_SCALE);
    parsed.clamp(MIN_UI_SCALE, MAX_UI_SCALE)
}

fn parse_bool_env(raw: Option<&str>) -> bool {
    match raw.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value)
            if value == "1"
                || value == "true"
                || value == "yes"
                || value == "on"
                || value == "enabled" =>
        {
            true
        }
        _ => false,
    }
}

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
    pub feedback_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptExit {
    Submitted(String),
    Canceled,
    TimedOut,
}

pub struct PromptSession {
    dialog: PromptDialog,
}

struct PromptDialog {
    window: Window,
    renderer: Renderer,
    gc: u32,
    keymap: Option<X11Keymap>,
    strings: PromptStrings,
    ui_scale: f32,
    card_padding: i32,
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
    focus_ring: Color,
    error_blink_on: bool,
    last_blink_toggle: Instant,
    error_flash_until: Option<Instant>,
}

#[derive(Debug, Clone)]
struct X11Keymap {
    min_keycode: u8,
    max_keycode: u8,
    keysyms_per_keycode: u8,
    keysyms: Vec<u32>,
}

impl X11Keymap {
    fn load(conn: &Connection) -> Result<Self> {
        let setup = conn.inner().setup();
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let count = max_keycode.saturating_sub(min_keycode).saturating_add(1);
        let reply = conn
            .inner()
            .get_keyboard_mapping(min_keycode, count)
            .context("failed to query X11 keyboard mapping")?
            .reply()
            .context("failed to read X11 keyboard mapping reply")?;
        Ok(Self {
            min_keycode,
            max_keycode,
            keysyms_per_keycode: reply.keysyms_per_keycode,
            keysyms: reply.keysyms,
        })
    }

    fn char_for_event(&self, event: &KeyEvent) -> Option<char> {
        if event.keycode < self.min_keycode || event.keycode > self.max_keycode {
            return None;
        }
        let stride = self.keysyms_per_keycode as usize;
        if stride == 0 {
            return None;
        }
        let offset = (event.keycode - self.min_keycode) as usize * stride;
        let symbols = self.keysyms.get(offset..offset + stride)?;

        // Level 0/1 captures the common unshifted/shifted mapping for the active layout.
        let level = if event.modifiers.shift { 1 } else { 0 };
        let keysym = symbols
            .get(level)
            .copied()
            .filter(|sym| *sym != 0)
            .or_else(|| symbols.first().copied())
            .unwrap_or(0);
        if keysym == 0 {
            return None;
        }
        let mut ch = keysym_to_char(keysym)?;
        if ch.is_ascii_alphabetic() {
            let upper = event.modifiers.shift ^ event.modifiers.caps_lock;
            ch = if upper {
                ch.to_ascii_uppercase()
            } else {
                ch.to_ascii_lowercase()
            };
        }
        Some(ch)
    }
}

fn keysym_to_char(keysym: u32) -> Option<char> {
    if (0x20..=0x7e).contains(&keysym) || (0xA0..=0xFF).contains(&keysym) {
        return char::from_u32(keysym);
    }
    if (0x0100_0000..=0x0110_FFFF).contains(&keysym) {
        return char::from_u32(keysym - 0x0100_0000);
    }
    None
}

pub fn run_prompt_dialog(request: PromptRequest) -> Result<PromptExit> {
    let mut session = PromptSession::connect()?;
    session.run(request)
}

impl PromptSession {
    pub fn connect() -> Result<Self> {
        let request = PromptRequest {
            message: String::new(),
            mode: PromptMode::Secret,
            timeout_secs: 0,
            tone: PromptTone::Default,
            feedback_only: false,
        };
        let ui = PromptUiConfig::from_env();
        let strings = PromptStrings::for_locale(&ui.locale);
        let dialog_width = ((BASE_DIALOG_WIDTH as f32) * ui.ui_scale).round() as u32;
        let dialog_height = ((BASE_DIALOG_HEIGHT as f32) * ui.ui_scale).round() as u32;

        let conn = Connection::connect(None).context("failed to connect to X11 display")?;
        let (x, y) = centered_position(&conn, dialog_width, dialog_height);
        let window = Window::create(
            conn.clone(),
            WindowConfig::dialog()
                .title(strings.title_auth_required)
                .class("garcard")
                .position(x, y)
                .size(dialog_width, dialog_height)
                .transparent(true)
                .modal(true),
        )
        .context("failed to create prompt window")?;
        window.focus().context("failed to focus prompt window")?;

        let dialog = PromptDialog::new(window, request, &ui)?;
        Ok(Self { dialog })
    }

    pub fn run(&mut self, request: PromptRequest) -> Result<PromptExit> {
        self.dialog.run_request(request)
    }

    pub fn show_feedback(
        &mut self,
        message: &str,
        tone: PromptTone,
        timeout_secs: u64,
    ) -> Result<()> {
        let _ = self.run(PromptRequest {
            message: message.to_string(),
            mode: PromptMode::Plain,
            timeout_secs,
            tone,
            feedback_only: true,
        })?;
        Ok(())
    }
}

impl std::fmt::Debug for PromptSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PromptSession(..)")
    }
}

impl PromptDialog {
    fn run_request(&mut self, request: PromptRequest) -> Result<PromptExit> {
        self.begin_request(request);
        self.window
            .focus()
            .context("failed to focus prompt window")?;

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

    fn begin_request(&mut self, request: PromptRequest) {
        scrub_string(&mut self.input);
        self.cursor = 0;
        self.request = request;
        self.exit = None;
        self.remaining_secs = None;
        self.error_blink_on = true;
        self.last_blink_toggle = Instant::now();
        self.error_flash_until =
            if !self.request.feedback_only && self.request.tone == PromptTone::Error {
                Some(Instant::now() + RETRY_ERROR_FLASH_DURATION)
            } else {
                None
            };
        self.deadline = if self.request.timeout_secs > 0 {
            Some(Instant::now() + Duration::from_secs(self.request.timeout_secs))
        } else {
            None
        };
    }
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
    fn new(window: Window, request: PromptRequest, ui: &PromptUiConfig) -> Result<Self> {
        let mut theme = Theme::dark();
        theme.font_family = std::env::var("GARCARD_PROMPT_FONT_FAMILY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Noto Sans".to_string());
        theme.font_size = (14.0_f64 * ui.ui_scale as f64).max(12.0);
        let palette = PromptPalette::from_high_contrast(ui.high_contrast)?;
        let strings = PromptStrings::for_locale(&ui.locale);
        let size = window.size();
        let renderer = Renderer::with_theme(size.width, size.height, theme)?;

        let conn = window.connection();
        let keymap = match X11Keymap::load(conn) {
            Ok(keymap) => Some(keymap),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "Failed to load X11 keyboard mapping; falling back to toolkit key translation"
                );
                None
            }
        };
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
            keymap,
            strings,
            ui_scale: ui.ui_scale,
            card_padding: ((BASE_CARD_PADDING as f32) * ui.ui_scale).round().max(12.0) as i32,
            request,
            input: String::new(),
            cursor: 0,
            exit: None,
            deadline,
            remaining_secs: None,
            backdrop: palette.backdrop,
            card_background: palette.card_background,
            card_border: palette.card_border,
            accent: palette.accent,
            success: palette.success,
            error: palette.error,
            focus_ring: palette.focus_ring,
            error_blink_on: true,
            last_blink_toggle: Instant::now(),
            error_flash_until: None,
        })
    }

    fn scaled_i32(&self, value: i32) -> i32 {
        ((value as f32) * self.ui_scale).round() as i32
    }

    fn scaled_u32(&self, value: u32) -> u32 {
        ((value as f32) * self.ui_scale).round().max(1.0) as u32
    }

    fn scaled_f64(&self, value: f64) -> f64 {
        value * self.ui_scale as f64
    }

    fn timeout_text(&self, remaining_secs: u64) -> String {
        format!("{} {}s", self.strings.timeout_label, remaining_secs)
    }

    fn refresh_timeout(&mut self) -> bool {
        let mut changed = false;
        let now = Instant::now();

        if let Some(until) = self.error_flash_until {
            if now >= until {
                self.error_flash_until = None;
                self.request.tone = PromptTone::Default;
                self.error_blink_on = true;
                changed = true;
            }
        }

        let Some(deadline) = self.deadline else {
            if self.request.tone == PromptTone::Error
                && now.duration_since(self.last_blink_toggle) >= ERROR_BLINK_INTERVAL
            {
                self.last_blink_toggle = now;
                self.error_blink_on = !self.error_blink_on;
                changed = true;
            }
            return changed;
        };
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
        let resolved_char = if matches!(key_event.key, Key::Char(_)) {
            self.keymap
                .as_ref()
                .and_then(|keymap| keymap.char_for_event(key_event))
        } else {
            None
        };
        apply_key_event(
            &mut self.input,
            &mut self.cursor,
            &mut self.exit,
            self.request.feedback_only,
            key_event,
            resolved_char,
        );
    }

    fn render(&mut self) -> Result<()> {
        let size = self.renderer.size();
        let width = size.width as i32;
        let height = size.height as i32;
        let theme = self.renderer.theme().clone();
        let pad = self.card_padding;
        let body_inset = self.scaled_i32(16);
        let title_top = self.scaled_i32(14);
        let message_top = self.scaled_i32(46);
        let label_top = self.scaled_i32(108);
        let input_top = self.scaled_i32(126);
        let input_height = self.scaled_u32(40);
        let input_inner_x = self.scaled_i32(10);
        let input_inner_y = self.scaled_i32(12);
        let footer_top_offset = self.scaled_i32(26);
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
            pad,
            pad,
            (width - pad * 2) as u32,
            (height - pad * 2) as u32,
        );
        self.renderer
            .fill_rounded_rect(card_rect, self.scaled_f64(12.0), self.card_background)?;
        self.renderer.stroke_rounded_rect(
            card_rect,
            self.scaled_f64(12.0),
            self.card_border,
            self.scaled_f64(2.0),
        )?;

        let title_style = TextStyle::new()
            .font_family(theme.font_family.clone())
            .font_size(self.scaled_f64(18.0))
            .color(theme.foreground);
        self.renderer.text(
            self.strings.title_auth_required,
            (pad + body_inset) as f64,
            (pad + title_top) as f64,
            &title_style,
        )?;

        let message_style = TextStyle::new()
            .font_family(theme.font_family.clone())
            .font_size(self.scaled_f64(13.0))
            .color(theme.item_description)
            .max_width(card_rect.width as i32 - body_inset * 2)
            .wrap(true)
            .ellipsize(true);
        self.renderer.text(
            &self.request.message,
            (pad + body_inset) as f64,
            (pad + message_top) as f64,
            &message_style,
        )?;

        let input_label = match self.request.mode {
            PromptMode::Secret => self.strings.label_password,
            PromptMode::Plain => self.strings.label_response,
        };
        let label_style = TextStyle::new()
            .font_family(theme.font_family.clone())
            .font_size(self.scaled_f64(12.0))
            .color(theme.input_placeholder);
        self.renderer.text(
            input_label,
            (pad + body_inset) as f64,
            (pad + label_top) as f64,
            &label_style,
        )?;

        let input_rect = Rect::new(
            pad + body_inset,
            pad + input_top,
            (width - (pad + body_inset) * 2) as u32,
            input_height,
        );
        let focus_rect = Rect::new(
            input_rect.x - self.scaled_i32(2),
            input_rect.y - self.scaled_i32(2),
            input_rect.width + self.scaled_u32(4),
            input_rect.height + self.scaled_u32(4),
        );
        self.renderer.stroke_rounded_rect(
            focus_rect,
            self.scaled_f64(10.0),
            self.focus_ring.with_alpha(0.75),
            self.scaled_f64(1.2),
        )?;
        self.renderer.fill_rounded_rect(
            input_rect,
            self.scaled_f64(8.0),
            theme.input_background,
        )?;
        self.renderer.stroke_rounded_rect(
            input_rect,
            self.scaled_f64(8.0),
            accent.with_alpha(0.85),
            self.scaled_f64(1.5),
        )?;

        let display_input = display_value(&self.input, self.request.mode);
        let input_style = TextStyle::new()
            .font_family(theme.font_family.clone())
            .font_size(self.scaled_f64(14.0))
            .color(theme.input_foreground);
        let input_y = input_rect.y + input_inner_y;
        let input_x = input_rect.x + input_inner_x;
        self.renderer
            .text(&display_input, input_x as f64, input_y as f64, &input_style)?;

        let cursor_prefix = display_prefix(&self.input, self.cursor, self.request.mode);
        if self.request.tone == PromptTone::Default {
            let cursor_size = self.renderer.measure_text(&cursor_prefix, &input_style)?;
            let cursor_x = input_x + cursor_size.width as i32;
            self.renderer.fill_rect(
                Rect::new(
                    cursor_x,
                    input_rect.y + self.scaled_i32(8),
                    self.scaled_u32(2),
                    input_rect.height.saturating_sub(self.scaled_u32(16)),
                ),
                accent,
            )?;
        }

        let footer_style = TextStyle::new()
            .font_family(theme.font_family.clone())
            .font_size(self.scaled_f64(12.0))
            .color(theme.item_description);
        let footer_text = if self.request.feedback_only {
            self.strings.footer_wait
        } else {
            self.strings.footer_controls
        };
        self.renderer.text(
            footer_text,
            (pad + body_inset) as f64,
            (height - pad - footer_top_offset) as f64,
            &footer_style,
        )?;

        if let Some(remaining) = self.remaining_secs {
            let timer_text = self.timeout_text(remaining);
            let timer_style = TextStyle::new()
                .font_family(theme.font_family.clone())
                .font_size(self.scaled_f64(12.0))
                .color(accent);
            let timer_size = self.renderer.measure_text(&timer_text, &timer_style)?;
            self.renderer.text(
                &timer_text,
                (width - pad - timer_size.width as i32 - body_inset) as f64,
                (height - pad - footer_top_offset) as f64,
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

fn apply_key_event(
    input: &mut String,
    cursor: &mut usize,
    exit: &mut Option<PromptExit>,
    feedback_only: bool,
    key_event: &KeyEvent,
    resolved_char: Option<char>,
) {
    if feedback_only {
        // Feedback dialogs are transient; ignore keypresses so submit/escape
        // from the previous prompt cannot dismiss success/error feedback early.
        return;
    }

    match key_event.key {
        Key::Escape => {
            *exit = Some(PromptExit::Canceled);
        }
        Key::Return => {
            let submitted = std::mem::take(input);
            *cursor = 0;
            *exit = Some(PromptExit::Submitted(submitted));
        }
        Key::Left => {
            if *cursor > 0 {
                *cursor -= 1;
            }
        }
        Key::Right => {
            if *cursor < input.chars().count() {
                *cursor += 1;
            }
        }
        Key::Home => {
            *cursor = 0;
        }
        Key::End => {
            *cursor = input.chars().count();
        }
        Key::Backspace => {
            remove_char_before(input, cursor);
        }
        Key::Delete => {
            remove_char_at(input, *cursor);
        }
        Key::Space => {
            if key_event.modifiers.is_empty() || key_event.modifiers.shift {
                insert_char_at(input, *cursor, ' ');
                *cursor += 1;
            }
        }
        Key::Char(ch) => {
            if key_event.modifiers.ctrl || key_event.modifiers.alt || key_event.modifiers.super_key
            {
                return;
            }
            let resolved = resolved_char.unwrap_or(ch);
            if resolved.is_control() {
                return;
            }
            insert_char_at(input, *cursor, resolved);
            *cursor += 1;
        }
        _ => {}
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
    use gartk_core::Modifiers;

    fn key_event(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            keycode: 0,
            modifiers: Modifiers::NONE,
            pressed: true,
        }
    }

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

    #[test]
    fn keysym_to_char_maps_ascii_and_unicode() {
        assert_eq!(keysym_to_char('A' as u32), Some('A'));
        assert_eq!(keysym_to_char(0x0100_03B1), Some('α'));
        assert_eq!(keysym_to_char(0), None);
    }

    #[test]
    fn locale_bucket_supports_spanish_and_defaults_to_english() {
        assert_eq!(locale_bucket("es_ES.UTF-8"), "es");
        assert_eq!(locale_bucket("en_US.UTF-8"), "en");
        assert_eq!(locale_bucket("C"), "en");
    }

    #[test]
    fn prompt_scale_from_env_clamps_range() {
        assert_eq!(prompt_scale_from_env(Some("0.2")), MIN_UI_SCALE);
        assert_eq!(prompt_scale_from_env(Some("1.5")), 1.5);
        assert_eq!(prompt_scale_from_env(Some("9.9")), MAX_UI_SCALE);
        assert_eq!(prompt_scale_from_env(None), DEFAULT_UI_SCALE);
    }

    #[test]
    fn parse_bool_env_accepts_common_truthy_values() {
        assert!(parse_bool_env(Some("1")));
        assert!(parse_bool_env(Some("TRUE")));
        assert!(parse_bool_env(Some("enabled")));
        assert!(!parse_bool_env(Some("0")));
        assert!(!parse_bool_env(None));
    }

    #[test]
    fn prompt_strings_localize_known_labels() {
        let english = PromptStrings::for_locale("en_US.UTF-8");
        assert_eq!(english.title_auth_required, "Authentication Required");
        assert_eq!(english.footer_controls, "Enter submit   Esc cancel");

        let spanish = PromptStrings::for_locale("es_ES.UTF-8");
        assert_eq!(spanish.label_password, "Contrasena");
        assert_eq!(spanish.footer_wait, "Espere");
    }

    #[test]
    fn apply_key_event_submits_and_clears_input_on_return() {
        let mut input = "secret".to_string();
        let mut cursor = input.chars().count();
        let mut exit = None;

        apply_key_event(
            &mut input,
            &mut cursor,
            &mut exit,
            false,
            &key_event(Key::Return),
            None,
        );

        assert_eq!(cursor, 0);
        assert!(input.is_empty());
        assert_eq!(exit, Some(PromptExit::Submitted("secret".to_string())));
    }

    #[test]
    fn apply_key_event_ignores_keys_for_feedback_only_dialogs() {
        let mut input = "ok".to_string();
        let mut cursor = 1;
        let mut exit = None;

        apply_key_event(
            &mut input,
            &mut cursor,
            &mut exit,
            true,
            &key_event(Key::Escape),
            None,
        );

        assert_eq!(input, "ok");
        assert_eq!(cursor, 1);
        assert!(exit.is_none());
    }

    #[test]
    fn apply_key_event_respects_navigation_and_edit_shortcuts() {
        let mut input = "abcd".to_string();
        let mut cursor = 2;
        let mut exit = None;

        apply_key_event(
            &mut input,
            &mut cursor,
            &mut exit,
            false,
            &key_event(Key::Left),
            None,
        );
        assert_eq!(cursor, 1);

        apply_key_event(
            &mut input,
            &mut cursor,
            &mut exit,
            false,
            &key_event(Key::Backspace),
            None,
        );
        assert_eq!(input, "bcd");
        assert_eq!(cursor, 0);

        apply_key_event(
            &mut input,
            &mut cursor,
            &mut exit,
            false,
            &key_event(Key::End),
            None,
        );
        assert_eq!(cursor, 3);

        apply_key_event(
            &mut input,
            &mut cursor,
            &mut exit,
            false,
            &key_event(Key::Delete),
            None,
        );
        assert_eq!(input, "bcd");

        apply_key_event(
            &mut input,
            &mut cursor,
            &mut exit,
            false,
            &key_event(Key::Home),
            None,
        );
        assert_eq!(cursor, 0);
    }

    #[test]
    fn apply_key_event_ignores_ctrl_shortcuts_and_control_chars() {
        let mut input = String::new();
        let mut cursor = 0;
        let mut exit = None;

        let mut ctrl_event = key_event(Key::Char('x'));
        ctrl_event.modifiers.ctrl = true;
        apply_key_event(&mut input, &mut cursor, &mut exit, false, &ctrl_event, None);
        assert!(input.is_empty());
        assert_eq!(cursor, 0);

        apply_key_event(
            &mut input,
            &mut cursor,
            &mut exit,
            false,
            &key_event(Key::Char('a')),
            Some('\n'),
        );
        assert!(input.is_empty());
        assert_eq!(cursor, 0);
    }
}
