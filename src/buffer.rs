use std::ops::Range;

use log::debug;
use tinyvec::{ArrayVec, array_vec};

use crate::CursorDirection;

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
    pub row_off: usize,
    pub col_off: usize,
    name: String,
    cur_line: usize,
    cur_col: usize,
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            row: vec![],
            row_off: 0,
            col_off: 0,
            cur_line: 0,
            cur_col: 0,
            name: String::new(),
        }
    }

    pub fn read(name: String, s: &str) -> Self {
        let row = s.lines().map(|line| Row::new(line.to_owned())).collect();
        Self {
            row,
            row_off: 0,
            col_off: 0,
            cur_col: 0,
            cur_line: 0,
            name,
        }
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

    fn row_len(&self, row: usize) -> usize {
        self.row.get(row).map(Row::render_len).unwrap_or(0)
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

    fn cx_to_rendered(&self, cx: u16) -> u16 {
        let Some(row) = self.row.get(self.cur_line) else {
            return 0;
        };

        row.cx_to_rendered(cx)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn move_cursor(&mut self, c: CursorDirection, rows: u16, cols: u16) {
        use CursorDirection as C;

        match c {
            C::Up => self.cur_line = self.cur_line.saturating_sub(1),
            C::Down => {
                self.cur_line = (self.cur_line + 1).clamp(0, self.row.len().saturating_sub(1))
            }
            C::Left => self.cur_col = self.cur_col.saturating_sub(1),
            C::Right => self.cur_col += 1,
        }

        self.cur_col = self
            .cur_col
            .clamp(0, self.row_len(self.cur_line).saturating_sub(1));

        let cx = (self.cur_col as isize - self.col_off as isize) as i32;
        let cy = (self.cur_line as isize - self.row_off as isize) as i32;

        self.fit_pos(cx, cy, rows, cols);

        debug!("cx={}, cy={}, rows={}, cols={}", cx, cy, rows, cols);
    }

    /// where to place the cursor (rows x cols coordinates)
    /// returns (y, x)
    pub fn cursor(&self) -> (u16, u16) {
        (
            (self.cur_line - self.row_off).try_into().unwrap(),
            self.cx_to_rendered((self.cur_col - self.col_off).try_into().unwrap()),
        )
    }

    /// returns (line, col)
    pub fn position(&self) -> (usize, usize) {
        (self.cur_line, self.cur_col)
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CursorDirection as C;

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
                cur_line: 0,
                cur_col: 0,
            }
        );
    }

    type Position = (usize, usize);
    type Cursor = (u16, u16);
    fn enact(
        buf: &mut Buffer,
        rows: u16,
        cols: u16,
        actions: &[(CursorDirection, Position, Cursor)],
    ) {
        for (i, &(action, pos, cursor)) in actions.iter().enumerate() {
            buf.move_cursor(action, rows, cols);
            assert_eq!(buf.position(), pos, "position: #{i} {action:?}");
            assert_eq!(buf.cursor(), cursor, "cursor: #{i} {action:?}");
        }
    }

    fn new_buf(s: &str) -> Buffer {
        let name = "foo.vpr".to_owned();
        Buffer::read(name, textwrap::dedent(s).trim())
    }

    #[test]
    fn move_cursor() {
        let rows = 24;
        let cols = 80;
        let mut buf = new_buf(
            r#"
             foo bar
             baz
             blah
             "#,
        );

        enact(
            &mut buf,
            rows,
            cols,
            &[
                (C::Left, (0, 0), (0, 0)),
                (C::Right, (0, 1), (0, 1)),
                (C::Left, (0, 0), (0, 0)),
                (C::Down, (1, 0), (1, 0)),
                (C::Right, (1, 1), (1, 1)),
                (C::Right, (1, 2), (1, 2)),
                (C::Right, (1, 2), (1, 2)),
                (C::Right, (1, 2), (1, 2)),
            ],
        );
    }

    #[test]
    fn move_cursor_scroll() {
        let rows = 3;
        let cols = 80;
        let mut buf = new_buf(
            r#"
                line 1
                line 2
                line 3
                line 4
                line 5
                "#,
        );

        enact(
            &mut buf,
            rows,
            cols,
            &[
                (C::Down, (1, 0), (1, 0)),
                (C::Down, (2, 0), (2, 0)),
                (C::Down, (3, 0), (2, 0)),
                (C::Down, (4, 0), (2, 0)),
                (C::Down, (4, 0), (2, 0)),
                (C::Up, (3, 0), (1, 0)),
                (C::Up, (2, 0), (0, 0)),
                (C::Up, (1, 0), (0, 0)),
                (C::Up, (0, 0), (0, 0)),
            ],
        );
    }

    #[test]
    fn move_cursor_end_of_line() {
        let rows = 3;
        let cols = 80;
        let mut buf = new_buf(
            r#"
            long line 1
            line 2
            "#,
        );

        for _ in 0..11 {
            buf.move_cursor(C::Right, rows, cols);
        }
        assert_eq!(buf.position(), (0, 10));
        assert_eq!(buf.cursor(), (0, 10));

        enact(&mut buf, rows, cols, &[(C::Down, (1, 5), (1, 5))]);
    }

    #[test]
    fn move_cursor_scroll_end_of_line() {
        let rows = 3;
        let cols = 7;
        let mut buf = new_buf(
            r#"
            long line 1
            line 2
            "#,
        );

        for _ in 0..11 {
            buf.move_cursor(C::Right, rows, cols);
        }
        assert_eq!(buf.position(), (0, 10));
        assert_eq!(buf.cursor(), (0, 6));

        enact(&mut buf, rows, cols, &[(C::Down, (1, 5), (1, 5))]);
    }
}
