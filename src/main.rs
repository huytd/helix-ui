use std::{fmt::Display, thread};

use druid::{
    piet::{PietText, Text, TextAttribute, TextLayoutBuilder},
    widget::Painter,
    AppDelegate, AppLauncher, Application, Color, Data, ExtEventSink, FontDescriptor, FontFamily,
    Handled, Rect, RenderContext, Selector, Target, TextLayout, Widget, WindowDesc,
};

use once_cell::sync::OnceCell;
use portable_pty::{native_pty_system, CommandBuilder, PtyPair, PtySize};
use vte::{Params, Parser, Perform};

#[derive(Clone, Copy, Debug, Data, PartialEq, Eq)]
struct Cell {
    content: char,
    bg: Option<Color>,
    fg: Option<Color>,
    // TODO: inverse
}

impl Cell {
    pub fn new() -> Self {
        Self {
            content: ' ',
            bg: None,
            fg: None,
        }
    }

    pub fn set_content(&mut self, c: &char) {
        self.content = c.to_owned();
    }

    pub fn set_bg(&mut self, color: Option<(u16, u16, u16)>) {
        if let Some(color) = color {
            let r = color.0 as f64 / 255.0;
            let g = color.1 as f64 / 255.0;
            let b = color.2 as f64 / 255.0;
            self.bg = Some(Color::rgb(r, g, b));
        } else {
            self.bg = None;
        }
    }

    pub fn set_fg(&mut self, color: Option<(u16, u16, u16)>) {
        if let Some(color) = color {
            let r = color.0 as f64 / 255.0;
            let g = color.1 as f64 / 255.0;
            let b = color.2 as f64 / 255.0;
            self.fg = Some(Color::rgb(r, g, b));
        } else {
            self.fg = None;
        }
    }
}

struct HelixUIState {
    grid: Vec<Vec<Cell>>,
    cursor: (usize, usize),
}

const UPDATE_UI: Selector<HelixUIState> = Selector::new("helix-ui.update-screen");
const DESTROY_UI: Selector = Selector::new("helix-ui.destroy");

pub const NEWLINE_CHAR: char = 10 as char;
pub const SPACE_CHAR: char = 32 as char;
pub const TAB_CHAR: char = 9 as char;
pub const BACK_CHAR: char = 8 as char;
pub const ESC_CHAR: char = 27 as char;
pub const CR_CHAR: char = 13 as char;
pub const BELL_CHAR: char = 7 as char;

struct ANSIParser {
    row: usize,
    col: usize,
    grid: Vec<Vec<Cell>>,
    current_fg: Option<(u16, u16, u16)>,
    current_bg: Option<(u16, u16, u16)>,
}

impl ANSIParser {
    pub fn new() -> Self {
        Self {
            row: 0,
            col: 0,
            grid: vec![vec![Cell::new(); 81]; 25],
            current_fg: None,
            current_bg: None,
        }
    }
}

impl Perform for ANSIParser {
    fn print(&mut self, c: char) {
        self.grid[self.row][self.col].set_content(&c);
        self.grid[self.row][self.col].set_bg(self.current_bg);
        self.grid[self.row][self.col].set_fg(self.current_fg);
        self.col += 1;
    }

    fn execute(&mut self, _byte: u8) {}

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _c: char) {}

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}

    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], _ignore: bool, c: char) {
        // CSI sequences https://www.xfree86.org/current/ctlseqs.html
        // This list is better  https://wezfurlong.org/wezterm/escape-sequences.html
        match c {
            // TODO: Handle [CSI Ps; m] for colors
            // [CSI Ps; Ps H] sequences to control the cursor
            'm' => {
                let pm = params.iter().flatten().collect::<Vec<_>>();
                // println!("CSI :: {:?}", pm);

                let header = pm.iter().take(2).collect::<Vec<_>>();
                // [38; 2; R; G; B] = Foreground RGB
                // [48; 2; R; G; B] = Background RGB
                // [7] = Inverse On
                // [27] = Inverse Off
                // [39] = Foreground Default
                // [49] = Background Default
                // [0] = Reset
                match header[..] {
                    [38, 2] => {
                        self.current_fg = Some((*pm[2], *pm[3], *pm[4]));
                    }
                    [48, 2] => {
                        self.current_bg = Some((*pm[2], *pm[3], *pm[4]));
                    }
                    [39] => {
                        self.current_fg = None;
                    }
                    [49] => {
                        self.current_bg = None;
                    }
                    [0] => {
                        self.current_fg = None;
                        self.current_bg = None;
                    }
                    _ => {}
                }
            }
            'H' => {
                // cursor moving
                let mut p = params.iter();
                let row = p.next().unwrap().first().unwrap();
                let col = p.next().unwrap().first().unwrap();
                self.row = (*row).into();
                self.col = (*col).into();
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Data)]
struct AppState {
    #[data(eq)]
    grid: Vec<Vec<Cell>>,
    cursor_pos: (usize, usize),
}

impl Display for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for row in 0..25 {
            for col in 0..81 {
                write!(f, "{}", self.grid[row][col].content)?;
            }
            write!(f, "\n")?;
        }
        write!(f, "")
    }
}

fn ui() -> impl Widget<AppState> {
    Painter::new(|ctx, data: &AppState, env| {
        // Calculate a single cell's size
        let mut letter_layout = TextLayout::<String>::from_text("H");
        letter_layout.set_font(FontDescriptor::new(FontFamily::MONOSPACE).with_size(12.0));
        letter_layout.rebuild_if_needed(ctx.text(), env);
        let cell_size = letter_layout.size();
        let cell_width = cell_size.width;
        let cell_height = cell_size.height;

        // Draw mode
        let mode: String = data.grid[23][2..=4]
            .into_iter()
            .map(|cell| cell.content)
            .collect();

        // Draw text content
        for row in 0..=24 {
            let text_ctx: &mut PietText = ctx.text();
            let line_text = data.grid[row]
                .iter()
                .map(|cell| cell.content)
                .collect::<String>();
            let chars = line_text.chars().collect::<Vec<char>>();
            let mut byte_offset = 0;
            let mut layout_builder = text_ctx
                .new_text_layout(line_text)
                .font(FontFamily::MONOSPACE, 12.0)
                .text_color(Color::WHITE);
            for (col, cell) in data.grid[row].iter().enumerate() {
                let cell_rect = Rect::new(
                    cell_width * col as f64,
                    cell_height * row as f64,
                    cell_width * col as f64 + cell_width,
                    cell_height * row as f64 + cell_height,
                );
                let cell_brush = ctx.solid_brush(cell.bg.unwrap_or(Color::TRANSPARENT));
                ctx.fill(cell_rect, &cell_brush);

                // Draw
                let text_color = cell.fg.unwrap_or(Color::WHITE);
                let ch = chars[col];
                let ch_len = ch.len_utf8();
                layout_builder = layout_builder.range_attribute(
                    byte_offset..byte_offset + ch_len,
                    TextAttribute::TextColor(text_color),
                );
                byte_offset += ch_len;
            }

            let text_layout = layout_builder.build().unwrap();
            ctx.draw_text(&text_layout, (0., cell_height * row as f64));
        }

        // Draw cursor
        let (cursor_row, cursor_col) = data.cursor_pos;
        if cursor_row <= 24 && cursor_col <= 80 {
            let grid_rect = Rect::new(
                cell_width * cursor_col as f64,
                cell_height * cursor_row as f64,
                cell_width * cursor_col as f64 + cell_width,
                cell_height * cursor_row as f64 + cell_height,
            )
            .to_rounded_rect(2.0);
            // Text under cursor
            let mut cursor_text_layout =
                TextLayout::<String>::from_text(data.grid[cursor_row][cursor_col].content);
            cursor_text_layout.set_font(FontDescriptor::new(FontFamily::MONOSPACE).with_size(12.0));
            cursor_text_layout.set_text_color(Color::BLACK);
            cursor_text_layout.rebuild_if_needed(ctx.text(), env);
            if mode.eq("INS") {
                let cursor_brush = ctx.solid_brush(Color::rgb(1.0, 0.87, 0.01));
                ctx.stroke(grid_rect, &cursor_brush, 1.0);
            } else {
                let cursor_brush = ctx.solid_brush(Color::rgb(1.0, 0.87, 0.01));
                ctx.fill(grid_rect, &cursor_brush);
                cursor_text_layout.draw(
                    ctx,
                    (
                        cell_width * cursor_col as f64,
                        cell_height * cursor_row as f64,
                    ),
                );
            }
        }
    })
}

struct Delegate;

impl AppDelegate<AppState> for Delegate {
    fn command(
        &mut self,
        _ctx: &mut druid::DelegateCtx,
        _target: Target,
        cmd: &druid::Command,
        data: &mut AppState,
        _env: &druid::Env,
    ) -> druid::Handled {
        if let Some(ui_state) = cmd.get(UPDATE_UI) {
            data.grid = ui_state.grid.to_vec();
            data.cursor_pos = ui_state.cursor;
            return Handled::Yes;
        }
        if cmd.is(DESTROY_UI) {
            Application::global().quit();
            return Handled::Yes;
        }
        druid::Handled::No
    }

    fn event(
        &mut self,
        _ctx: &mut druid::DelegateCtx,
        _window_id: druid::WindowId,
        event: druid::Event,
        _data: &mut AppState,
        _env: &druid::Env,
    ) -> Option<druid::Event> {
        let original_event = event.clone();
        match event {
            druid::Event::KeyDown(key_event) => {
                let modifiers = key_event.mods;
                let c = match key_event.key {
                    druid::keyboard_types::Key::Escape => 27 as u8,
                    druid::keyboard_types::Key::Backspace => 8 as u8,
                    druid::keyboard_types::Key::Tab => 9 as u8,
                    druid::keyboard_types::Key::Enter => 13 as u8,
                    _ => {
                        let chs = key_event.key.to_string().chars().next().unwrap();
                        if chs.is_lowercase() && chs.is_alphabetic() && modifiers.ctrl() {
                            match chs {
                                'a' => 0x01 as u8,
                                'b' => 0x02 as u8,
                                'c' => 0x03 as u8,
                                'd' => 0x04 as u8,
                                'e' => 0x05 as u8,
                                'f' => 0x06 as u8,
                                'g' => 0x07 as u8,
                                'h' => 0x08 as u8,
                                'i' => 0x09 as u8,
                                'j' => 0x10 as u8,
                                'k' => 0x11 as u8,
                                'l' => 0x12 as u8,
                                'm' => 0x13 as u8,
                                'n' => 0x14 as u8,
                                'o' => 0x15 as u8,
                                'p' => 0x16 as u8,
                                'q' => 0x17 as u8,
                                'r' => 0x18 as u8,
                                's' => 0x19 as u8,
                                't' => 0x1a as u8,
                                'u' => 0x21 as u8,
                                'v' => 0x22 as u8,
                                'w' => 0x23 as u8,
                                'x' => 0x24 as u8,
                                'y' => 0x25 as u8,
                                'z' => 0x26 as u8,
                                _ => chs as u8,
                            }
                        } else {
                            key_event.key.legacy_charcode() as u8
                        }
                    }
                };
                if c != 0 {
                    if let Some(pair) = unsafe { PTY_WRITER.get_mut() } {
                        _ = pair.0.master.write(&[c as u8]);
                    }
                }
            }
            _ => {}
        }
        Some(original_event)
    }
}

static UI_EVENT_SINK: OnceCell<ExtEventSink> = OnceCell::new();

struct PtyWriter(PtyPair);
unsafe impl Send for PtyWriter {}
unsafe impl Sync for PtyWriter {}

static mut PTY_WRITER: OnceCell<PtyWriter> = OnceCell::new();

fn main() {
    let win = WindowDesc::new(ui())
        .title("helix-ui")
        .window_size((600., 380.));
    let app = AppLauncher::with_window(win);
    let event_sink = app.get_external_handle();
    _ = UI_EVENT_SINK.set(event_sink);

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let cmd = CommandBuilder::new("/usr/local/bin/hx");
    _ = pair.slave.spawn_command(cmd).unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    _ = unsafe { PTY_WRITER.set(PtyWriter(pair)) };

    thread::spawn(move || {
        let mut buf = [0u8; 128];
        let mut parser = Parser::new();
        let mut ansi = ANSIParser::new();
        while let Ok(len) = reader.read(&mut buf) {
            if len == 0 {
                break;
            }
            for b in &buf[0..len] {
                parser.advance(&mut ansi, *b);
            }
            buf = [0u8; 128];
            if let Some(sink) = UI_EVENT_SINK.get() {
                _ = sink.submit_command(
                    UPDATE_UI,
                    HelixUIState {
                        grid: ansi.grid.to_vec(),
                        cursor: (ansi.row, ansi.col),
                    },
                    Target::Auto,
                );
            }
        }
        // Helix exit
        if let Some(sink) = UI_EVENT_SINK.get() {
            _ = sink.submit_command(DESTROY_UI, (), Target::Auto);
        }
    });

    _ = app.delegate(Delegate).launch(AppState {
        grid: vec![vec![Cell::new(); 81]; 25],
        cursor_pos: (0, 0),
    });
}
