use std::{
    io::Write,
    ops::{ControlFlow, Range},
    os::fd::AsRawFd,
};

use anyhow::anyhow;
use log::{LevelFilter, debug};
use log4rs::{
    append::file::FileAppender,
    config::{Appender, Root},
    encode::pattern::PatternEncoder,
};
use terminal_size::terminal_size;
use termios::Termios;
use tinyvec::{ArrayVec, array_vec};

use crate::get_input::{GetChar, StdinSource};

mod get_input;

#[derive(Debug, PartialEq, Eq)]
pub struct Row {
    content: String,
    render: String,
}

fn to_hex(x: u8) -> (char, char) {
    let lo = x & 0xf;
    let hi = x >> 4;
    let hex_dig = |ch| if ch < 10 { b'0' + ch } else { b'a' + (ch - 10) };
    (hex_dig(hi) as char, hex_dig(lo) as char)
}

impl Row {
    fn rendered_char(ch: char) -> ArrayVec<[char; 16]> {
        match ch {
            '\t' => array_vec!(_ => ' ', ' ', ' ', ' '),
            ch if ch.is_ascii_control() => {
                let (a, b) = to_hex(ch as u8);
                array_vec!(_ => 'X', a, b)
            }
            ch => array_vec!(_ => ch),
        }
    }

    fn rendered(s: &str) -> String {
        let mut ret = String::new();
        for ch in s.chars() {
            for r in Self::rendered_char(ch) {
                ret.push(r)
            }
        }
        ret
    }
    pub fn new(content: String) -> Self {
        let render = Self::rendered(&content);
        Self { content, render }
    }

    pub fn render_len(&self) -> usize {
        self.render.chars().count()
    }

    pub fn content_len(&self) -> usize {
        self.content.chars().count()
    }

    fn cx_to_rendered(&self, cx: u16) -> u16 {
        self.content
            .chars()
            .take(cx.into())
            .map(|ch| Self::rendered_char(ch).len() as u16)
            .sum()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Buffer {
    row: Vec<Row>,
    row_off: usize,
    col_off: usize,
    name: String,
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            row: vec![],
            row_off: 0,
            col_off: 0,
            name: String::new(),
        }
    }

    pub fn row_len(&self, row: usize) -> Option<usize> {
        self.row.get(self.row_off + row).map(Row::render_len)
    }

    fn get_byte_range_from_char_range(s: &str, start: usize, end: usize) -> Range<usize> {
        let mut sb = None;
        let mut eb = s.len();
        for (i, (byte, _)) in s.char_indices().enumerate() {
            if i == start {
                sb = Some(byte)
            } else if i == end {
                eb = byte
            }
        }
        if let Some(sb) = sb { sb..eb } else { 0..0 }
    }

    pub fn get_row(&self, row: usize, width: usize) -> Option<&str> {
        self.row.get(self.row_off + row).map(|row| {
            let start = self.col_off;
            let end = self.col_off + width;
            &row.render[Self::get_byte_range_from_char_range(&row.render, start, end)]
        })
    }

    pub fn is_empty(&self) -> bool {
        self.row.is_empty()
    }

    pub fn len(&self) -> usize {
        self.row.len()
    }

    pub fn read(name: String, s: &str) -> Self {
        let row = s.lines().map(|line| Row::new(line.to_owned())).collect();
        Self {
            row,
            row_off: 0,
            col_off: 0,
            name,
        }
    }

    pub fn scroll(
        &mut self,
        c: CursorDirection,
        current_row: usize,
        screen_height: usize,
        screen_width: usize,
    ) {
        use CursorDirection as C;
        let max_row_off = self.len().saturating_sub(screen_height);
        let row_len = self.row_len(current_row).unwrap_or(0);
        let max_col_off = row_len.saturating_sub(screen_width);
        match c {
            C::Up => self.row_off = self.row_off.saturating_sub(1),
            C::Down => self.row_off = (self.row_off + 1).clamp(0, max_row_off),
            C::Left => self.col_off = self.col_off.saturating_sub(1),
            C::Right => self.col_off = (self.col_off + 1).clamp(0, max_col_off),
        }
    }

    /// cx, cy are the coordinates in the current window.
    /// requires cx+self.col_off >= 0 && cy+self.row_off >= 0
    ///
    /// rows, cols are the dimensions of the screen.
    ///
    /// returns the new virtual coordinates.
    ///
    /// ensures
    /// ret.0 < cols && ret.1 < rows
    pub fn fit_pos(&mut self, cx: i32, cy: i32, rows: u16, cols: u16) -> (u16, u16) {
        let row_len = self
            .row
            .get(self.row_off.checked_add_signed(cy as isize).unwrap_or(0))
            .map(Row::render_len)
            .unwrap_or(0);

        // if (0 <= cx && cx < cols.into() && (cx as usize) < row_len) && (0 <= cy && cy < rows.into())
        // {
        //     // no scroll needed
        //     return (cx as u16, cy as u16);
        // }

        let max_row_off = self.len().saturating_sub(rows as usize);

        let max_col_off = row_len.saturating_sub(cols as usize);

        let mut cy = cy;
        let mut cx = cx;
        if cy >= rows.into() {
            // need to scroll down
            let scroll_by = cy - rows as i32 + 1;
            self.row_off += scroll_by as usize;
            cy = rows as i32 - 1;
        }

        if cy < 0 {
            // need to scroll up
            let scroll_by = -cy;
            self.row_off = self.row_off.saturating_sub(scroll_by as usize);
            cy = 0;
        }

        if cx >= cols.into() {
            let scroll_by = cx - cols as i32 + 1;
            self.col_off += scroll_by as usize;
            cx = cols as i32 - 1;
        }

        if cx < 0 {
            let scroll_by = -cx;
            self.col_off = self.col_off.saturating_sub(scroll_by as usize);
            cx = 0;
        }

        self.row_off = self.row_off.clamp(0, max_row_off);
        self.col_off = self.col_off.clamp(0, max_col_off);
        cx = std::cmp::min(cx as usize, row_len.saturating_sub(1)) as i32;
        cy = std::cmp::min(cy as usize, self.len().saturating_sub(1)) as i32;

        (cx.try_into().unwrap(), cy.try_into().unwrap())
    }

    fn cx_to_rendered(&self, row: u16, cx: u16) -> u16 {
        let Some(row) = self.row.get(self.row_off + row as usize) else {
            return 0;
        };

        row.cx_to_rendered(cx)
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

pub struct EditorConfig {
    rows: u16,
    cols: u16,
    cx: u16,
    cy: u16,
    rendered_cx: u16,
    out_buf: Vec<u8>,
    buf: Buffer,
    getchar: GetChar<StdinSource>,
    status_message: Option<String>,
}

impl EditorConfig {
    pub fn init() -> anyhow::Result<Self> {
        let (cols, rows) = get_terminal_size().ok_or(anyhow!("no terminal size"))?;
        Ok(Self {
            rows: rows - 1,
            cols,
            cx: 0,
            cy: 0,
            rendered_cx: 0,
            status_message: None,
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
        self.status_message = Some(msg);
    }

    pub fn clear_message(&mut self) {
        self.status_message = None;
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

#[derive(Debug, PartialEq, Eq)]
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
    PageUp,
    PageDown,
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
    use CursorDirection as C;

    let (cx, cy) = (conf.cx as i32, conf.cy as i32);
    let (cx, cy) = match c {
        C::Up => (cx, cy - 1),
        C::Down => (cx, cy + 1),
        C::Left => (cx - 1, cy),
        C::Right => (cx + 1, cy),
    };
    (conf.cx, conf.cy) = conf.buf.fit_pos(cx, cy, conf.rows, conf.cols);
    conf.rendered_cx = conf.buf.cx_to_rendered(conf.cy, conf.cx);
    debug!(
        "cx={}, cy={}, rows={}, cols={}, rcx={}",
        conf.cx, conf.cy, conf.rows, conf.cols, conf.rendered_cx
    );
}

fn handle_input(conf: &mut EditorConfig, ch: Input) -> ControlFlow<()> {
    match ch {
        Input::Arrow(direction) => move_cursor(conf, direction),
        Input::Char(b'h') => move_cursor(conf, CursorDirection::Left),
        Input::Char(b'l') => move_cursor(conf, CursorDirection::Right),
        Input::Char(b'k') => move_cursor(conf, CursorDirection::Up),
        Input::Char(b'j') => move_cursor(conf, CursorDirection::Down),
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
        _ => {}
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

    write!(conf, "\x1b[{};{}H", conf.cy + 1, conf.rendered_cx + 1).unwrap();

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
    append(conf.buf.name.as_bytes());
    append(b" ");

    let mut buf = [0u8; 16];

    let _ = write!(
        &mut buf[..],
        "{}:{}",
        conf.buf.row_off + conf.cy as usize + 1,
        conf.buf.col_off + conf.cx as usize + 1
    );
    let pre = buf
        .iter()
        .enumerate()
        .find(|x| *x.1 == 0)
        .map(|x| x.0)
        .unwrap_or(buf.len());
    append(&buf[..pre]);

    if let Some(sm) = &conf.status_message {
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

    conf.set_message("hello world".into());

    match std::env::args().nth(1) {
        Some(file) => {
            let c = std::fs::read_to_string(&file)?;
            conf.buf = Buffer::read(file, &c);
        }
        None => conf.buf = Buffer::new(),
    }

    loop {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_read() {
        let name = "foo.vpr".to_owned();
        assert_eq!(
            Buffer::read(name.clone(), &"hello".repeat(200)),
            Buffer {
                name,
                row: vec![Row::new("hello".repeat(200))],
                row_off: 0,
                col_off: 0,
            }
        );
    }
}
