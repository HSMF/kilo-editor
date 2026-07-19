use std::{fmt::Display, io::Write, ops::ControlFlow, os::fd::AsRawFd};

use anyhow::anyhow;
use log::{LevelFilter, debug};
use log4rs::{
    append::file::FileAppender,
    config::{Appender, Root},
    encode::pattern::PatternEncoder,
};
use terminal_size::terminal_size;
use termios::Termios;

use crate::{
    buffer::Buffer,
    get_input::{GetChar, StdinSource},
};

mod buffer;
mod get_input;

struct StatusMessage {
    inner: Option<String>,
    alive_for: usize,
}

impl StatusMessage {
    fn new() -> Self {
        Self {
            inner: None,
            alive_for: 0,
        }
    }

    pub fn set_message(&mut self, m: String) {
        self.alive_for = 30;
        self.inner = Some(m);
    }

    pub fn tick(&mut self) {
        self.alive_for = self.alive_for.saturating_sub(1);
        if self.alive_for == 0 {
            self.inner = None;
        }
    }

    pub fn msg(&self) -> Option<&str> {
        self.inner.as_deref()
    }
}

pub struct EditorConfig {
    rows: u16,
    cols: u16,
    out_buf: Vec<u8>,
    buf: Buffer,
    getchar: GetChar<StdinSource>,
    status_message: StatusMessage,
}

impl EditorConfig {
    pub fn init() -> anyhow::Result<Self> {
        let (cols, rows) = get_terminal_size().ok_or(anyhow!("no terminal size"))?;
        Ok(Self {
            rows: rows - 1,
            cols,
            status_message: StatusMessage::new(),
            out_buf: Vec::new(),
            buf: Buffer::new(),
            getchar: GetChar::new(StdinSource),
        })
    }

    pub fn append(&mut self, s: impl AsRef<[u8]>) {
        self.out_buf.extend_from_slice(s.as_ref());
    }

    pub fn flush(&mut self) -> anyhow::Result<()> {
        std::io::stdout().write_all(&self.out_buf)?;
        std::io::stdout().flush()?;
        self.out_buf.clear();
        Ok(())
    }

    pub fn set_message(&mut self, msg: String) {
        self.status_message.set_message(msg);
    }
}

impl std::io::Write for EditorConfig {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.out_buf.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out_buf.flush()
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CursorDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    Char(u8),
    Arrow(CursorDirection),
    Enter,
    Backspace,
    PageUp,
    PageDown,
    Escape,
}

const fn ctrl_key(x: u8) -> u8 {
    x & 0x1f
}

fn enter_raw_mode() -> Termios {
    let fd = std::io::stdin().as_raw_fd();
    let mut termios = Termios::from_fd(fd).unwrap();
    termios::tcgetattr(fd, &mut termios).unwrap();
    let orig = termios;
    termios.c_oflag &= !termios::OPOST;
    termios.c_lflag &= !(termios::ECHO | termios::ICANON | termios::ISIG);
    termios.c_cc[termios::VMIN] = 0;
    termios.c_cc[termios::VTIME] = 1;

    termios::tcsetattr(fd, termios::TCSAFLUSH, &termios).unwrap();
    orig
}

fn move_cursor(conf: &mut EditorConfig, c: CursorDirection) {
    conf.buf.move_cursor(c, conf.rows, conf.cols);
}

fn handle_input(conf: &mut EditorConfig, ch: Input) -> ControlFlow<()> {
    match ch {
        Input::Arrow(direction) => move_cursor(conf, direction),
        // Input::Char(b'h') => move_cursor(conf, CursorDirection::Left),
        // Input::Char(b'l') => move_cursor(conf, CursorDirection::Right),
        // Input::Char(b'k') => move_cursor(conf, CursorDirection::Up),
        // Input::Char(b'j') => move_cursor(conf, CursorDirection::Down),
        Input::Char(ch) if ch == ctrl_key(b'd') => return handle_input(conf, Input::PageDown),
        Input::Char(ch) if ch == ctrl_key(b'u') => return handle_input(conf, Input::PageUp),
        Input::PageDown => {
            for _ in 0..conf.rows / 2 {
                move_cursor(conf, CursorDirection::Down);
            }
        }
        Input::PageUp => {
            for _ in 0..conf.rows / 2 {
                move_cursor(conf, CursorDirection::Up);
            }
        }
        Input::Char(ch) if ch == ctrl_key(b'q') || ch == ctrl_key(b'w') => {
            return ControlFlow::Break(());
        }
        Input::Char(ch) if ch == ctrl_key(b's') => {
            let Some(path) = conf.buf.path() else {
                conf.set_message(format!("buffer {} has no path", conf.buf.name()));
                return ControlFlow::Continue(());
            };
            let path = path.to_owned();
            conf.buf.scrub();
            let s = conf.buf.save();
            let lines = conf.buf.num_lines();
            std::fs::write(path, &s).expect("cant write");
            conf.set_message(format!("{lines}L, {}B written", s.len()));
        }
        Input::Char(b'\x1b') => {
            debug!("ignoring stray escape char");
        }
        Input::Char(ch) => {
            debug!("inserting {ch:?}");
            conf.buf.insert_char(ch as char, conf.rows, conf.cols);
        }
        Input::Escape => {
            debug!("escape (the fate)")
        }
        Input::Backspace => conf.buf.delete_char(conf.rows, conf.cols),
        Input::Enter => conf.buf.add_newline(conf.rows, conf.cols),
    }
    ControlFlow::Continue(())
}

fn get_terminal_size() -> Option<(u16, u16)> {
    terminal_size().map(|(a, b)| (a.0, b.0))
}

fn refresh_screen(conf: &mut EditorConfig) {
    conf.append("\x1b[?25l");
    conf.append("\x1b[H");
    draw_rows(conf);
    draw_status_bar(conf);

    let (cy, cx) = conf.buf.cursor();
    write!(conf, "\x1b[{};{}H", cy + 1, cx + 1).unwrap();

    conf.append("\x1b[?25h");
    conf.flush().unwrap();
}

fn draw_status_bar(conf: &mut EditorConfig) {
    conf.append("\x1b[7m");
    let mut col = 0;
    let mut append = |s: &[u8]| {
        col += s.len();
        conf.out_buf.extend_from_slice(s);
    };

    append(b" ");
    append(conf.buf.name().as_bytes());
    append(b" ");

    let mut buf = [0u8; 16];

    let (cur_line, cur_col) = conf.buf.position();
    let _ = write!(&mut buf[..], "{}:{}", cur_line + 1, cur_col + 1);
    let pre = buf
        .iter()
        .enumerate()
        .find(|x| *x.1 == 0)
        .map(|x| x.0)
        .unwrap_or(buf.len());
    append(&buf[..pre]);

    if conf.buf.is_dirty() {
        append(b" [+]");
    }

    if let Some(sm) = &conf.status_message.msg() {
        append(b" ");
        append(sm.as_bytes());
    }

    for _ in (col as u16)..conf.cols {
        conf.append(" ");
    }
    conf.append("\x1b[m");
}

fn draw_rows(conf: &mut EditorConfig) {
    let rows = conf.rows;
    let cols = conf.cols;
    for y in 0..rows {
        if let Some(row) = conf.buf.get_row(y as usize, cols as usize) {
            conf.out_buf.extend_from_slice(row.as_bytes());
        } else if conf.buf.is_empty() && y == rows / 3 {
            let mut pad = cols / 2;
            if pad > 0 {
                conf.append("~");
                pad -= 1;
            }
            for _ in 0..pad {
                conf.append(" ");
            }
            write!(
                conf,
                "{} -- version {}",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION")
            )
            .unwrap();
        } else {
            conf.append("~");
        }
        conf.append("\x1b[K");
        conf.append("\r\n");
    }
}

fn main() -> anyhow::Result<()> {
    let logfile = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "[{level}] {file}:{line} {message}\n",
        )))
        .build("log/output.log")?;

    let config = log4rs::Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(logfile)))
        .build(
            Root::builder()
                .appender("logfile")
                .build(LevelFilter::Debug),
        )?;
    log4rs::init_config(config)?;

    let orig_termios = enter_raw_mode();
    let mut conf = EditorConfig::init()?;

    conf.set_message(format!(
        "welcome to {} v{}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    ));

    match std::env::args().nth(1) {
        Some(file) => {
            let c = std::fs::read_to_string(&file).unwrap_or_default();
            conf.buf = Buffer::read(file, &c);
        }
        None => conf.buf = Buffer::new(),
    }

    loop {
        conf.status_message.tick();
        refresh_screen(&mut conf);
        if let Some(ch) = conf.getchar.getch() {
            match handle_input(&mut conf, ch) {
                ControlFlow::Continue(_) => (),
                ControlFlow::Break(_) => {
                    conf.append("\x1b[2J");
                    conf.append("\x1b[H");
                    conf.flush()?;
                    break;
                }
            }
        }
    }

    termios::tcsetattr(
        std::io::stdin().as_raw_fd(),
        termios::TCSAFLUSH,
        &orig_termios,
    )
    .unwrap();
    Ok(())
}
