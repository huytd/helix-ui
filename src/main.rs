use std::{fmt::Display, thread};

use druid::{
    widget::Painter, AppDelegate, AppLauncher, Color, Data, ExtEventSink, FontDescriptor,
    FontFamily, Handled, Rect, RenderContext, Selector, Size, Target, TextLayout, Widget,
    WindowDesc,
};

use once_cell::sync::OnceCell;
use portable_pty::{native_pty_system, CommandBuilder, PtyPair, PtySize};
use vte::{Params, Parser, Perform};

struct HelixUIState {
    grid: Vec<Vec<char>>,
    cursor: (usize, usize),
}

const UPDATE_UI: Selector<HelixUIState> = Selector::new("helix-ui.update-screen");

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
    grid: Vec<Vec<char>>,
}

impl ANSIParser {
    pub fn new() -> Self {
        Self {
            row: 0,
            col: 0,
            grid: vec![vec![' '; 81]; 25],
        }
    }
}

impl Perform for ANSIParser {
    fn print(&mut self, c: char) {
        self.grid[self.row][self.col] = c;
        self.col += 1;
    }

    fn execute(&mut self, _byte: u8) {}

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _c: char) {}

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}

    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], _ignore: bool, c: char) {
        // CSI sequences https://www.xfree86.org/current/ctlseqs.html
        match c {
            // TODO: Handle [CSI Ps; m] for colors
            // [CSI Ps; Ps H] sequences to control the cursor
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
    grid: Vec<Vec<char>>,
    cursor_pos: (usize, usize),
}

impl Display for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for row in 0..25 {
            for col in 0..81 {
                write!(f, "{}", self.grid[row][col])?;
            }
            write!(f, "\n")?;
        }
        write!(f, "")
    }
}

fn ui() -> impl Widget<AppState> {
    Painter::new(|ctx, data: &AppState, env| {
        // Calculating text layout
        let text = data.to_string();
        let mut layout = TextLayout::<String>::from_text(text);
        layout.set_font(FontDescriptor::new(FontFamily::MONOSPACE).with_size(12.0));
        layout.set_text_color(Color::WHITE);
        layout.rebuild_if_needed(ctx.text(), env);

        // Calculate a single cell's size
        let mut letter_layout = TextLayout::<String>::from_text("H");
        letter_layout.set_font(FontDescriptor::new(FontFamily::MONOSPACE).with_size(12.0));
        letter_layout.rebuild_if_needed(ctx.text(), env);
        let cell_size = letter_layout.size();
        let cell_width = cell_size.width;
        let cell_height = cell_size.height;

        // Draw mode
        let mode: String = data.grid[23][2..=4].into_iter().collect();
        let mode_rect = Rect::new(
            cell_width * 2. - 5.0,
            cell_height * 23.,
            cell_width * 2. + 3. * cell_width + 5.0,
            cell_height * 23. + cell_height,
        )
        .to_rounded_rect(2.0);
        match mode.as_str() {
            "NOR" => {
                let normal_mode_brush = ctx.solid_brush(Color::rgb(0.48, 0.55, 0.55));
                ctx.fill(mode_rect, &normal_mode_brush);
            }
            "INS" => {
                let insert_mode_brush = ctx.solid_brush(Color::rgb(0.08, 0.60, 0.34));
                ctx.fill(mode_rect, &insert_mode_brush);
            }
            "SEL" => {
                let select_mode_brush = ctx.solid_brush(Color::rgb(0.51, 0.25, 0.51));
                ctx.fill(mode_rect, &select_mode_brush);
            }
            _ => {}
        }

        // Draw cursor
        let (cursor_row, cursor_col) = data.cursor_pos;
        let grid_rect = Rect::new(
            cell_width * cursor_col as f64,
            cell_height * cursor_row as f64,
            cell_width * cursor_col as f64 + cell_width,
            cell_height * cursor_row as f64 + cell_height,
        )
        .to_rounded_rect(2.0);
        if mode.eq("INS") {
            let cursor_brush = ctx.solid_brush(Color::rgb(1.0, 0.87, 0.01));
            ctx.stroke(grid_rect, &cursor_brush, 1.0);
        } else {
            let cursor_brush = ctx.solid_brush(Color::rgb(1.0, 0.87, 0.01).with_alpha(0.45));
            ctx.fill(grid_rect, &cursor_brush);
        }

        // Draw text content
        layout.draw(ctx, (0.0, 0.0));
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
            println!("CURSOR POS: {:?}", data.cursor_pos);
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
                let mut c: u8 = 0;
                let modifiers = key_event.mods;
                match key_event.key {
                    druid::keyboard_types::Key::Escape => {
                        c = 27 as u8;
                    }
                    druid::keyboard_types::Key::Backspace => {
                        c = 8 as u8;
                    }
                    druid::keyboard_types::Key::Tab => {
                        c = 9 as u8;
                    }
                    druid::keyboard_types::Key::Enter => {
                        c = 13 as u8;
                    }
                    _ => {
                        let chs = key_event.key.to_string().chars().next().unwrap();
                        if chs.is_lowercase() && chs.is_alphabetic() && modifiers.ctrl() {
                            c = match chs {
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
                            };
                        } else {
                            c = key_event.key.legacy_charcode() as u8;
                        }
                    }
                }
                if let Some(pair) = unsafe { PTY_WRITER.get_mut() } {
                    _ = pair.0.master.write(&[c as u8]);
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
    });

    _ = app.delegate(Delegate).launch(AppState {
        grid: vec![vec![' '; 81]; 25],
        cursor_pos: (0, 0),
    });
}
