use std::ops::Range;

use tinyvec::{ArrayVec, array_vec};

use crate::{CursorDirection, location::Location};

#[derive(Debug, PartialEq, Eq)]
pub struct Row {
    content: String,
    render: String,
    /// number of *visible* chars in `render`
    render_chars: usize,
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

    fn rendered(s: &str) -> (String, usize) {
        let mut ret = String::new();
        let mut len = 0;
        for ch in s.chars() {
            for r in Self::rendered_char(ch) {
                ret.push(r)
            }
            len += 1;
        }
        (ret, len)
    }
    pub fn new(content: String) -> Self {
        let (render, len) = Self::rendered(&content);
        Self {
            content,
            render_chars: len,
            render,
        }
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

    fn insert_char(&mut self, ch: char, cur_col: usize) {
        if cur_col == self.content_len() {
            self.content.push(ch);
            for ch in Self::rendered_char(ch) {
                self.render.push(ch);
            }
        } else {
            let idx = char_idx_to_byte_idx(&self.content, cur_col).expect("byte index exists");
            self.content.insert(idx, ch);
            self.recompute_rendered();
        }
    }

    fn recompute_rendered(&mut self) {
        (self.render, self.render_chars) = Self::rendered(&self.content);
    }

    fn delete_char(&mut self, cur_col: usize) {
        let (i, _) = self
            .content
            .char_indices()
            .take(cur_col)
            .last()
            .expect("cannot delete anything");
        self.content.remove(i);
        self.recompute_rendered();
    }

    fn append_row(&mut self, other: Row) {
        self.content += &other.content;
        self.render += &other.render;
        self.render_chars += other.render_chars;
    }

    fn split(&mut self, cur_col: usize) -> Row {
        let (i, _) = self
            .content
            .char_indices()
            .nth(cur_col)
            .unwrap_or((self.content.len(), ' '));
        let (before, after) = self.content.split_at(i);
        let after = after.to_owned();
        self.content.truncate(before.len());
        self.recompute_rendered();
        Row::new(after)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Buffer {
    row: Vec<Row>,
    row_off: usize,
    col_off: usize,
    name: String,
    path: Option<String>,
    cur_line: usize,
    cur_col: usize,
    dirty: bool,
}

fn get_byte_range_from_char_range(s: &str, start: usize, end: usize) -> Range<usize> {
    let mut sb = None;
    let mut eb = s.len();
    for (i, (byte, _)) in s.char_indices().enumerate() {
        if i == start {
            sb = Some(byte)
        }
        if i == end {
            eb = byte
        }
    }
    if let Some(sb) = sb { sb..eb } else { 0..0 }
}

fn char_idx_to_byte_idx(s: &str, idx: usize) -> Option<usize> {
    s.char_indices().nth(idx).map(|x| x.0)
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
            path: None,
            dirty: false,
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
            path: Some(name.clone()),
            name,
            dirty: false,
        }
    }

    /// mark as "not dirty"
    pub fn scrub(&mut self) {
        self.dirty = false;
    }

    pub fn save(&self) -> String {
        let len = self.row.iter().map(|x| x.content.len() + 1).sum();
        let mut ret = String::with_capacity(len);

        for row in self.row.iter() {
            ret.push_str(&row.content);
            ret.push('\n');
        }

        ret
    }

    fn row_len(&self, row: usize) -> usize {
        self.row.get(row).map(Row::content_len).unwrap_or(0)
    }

    pub fn get_row(&self, row: usize) -> Option<&str> {
        self.row.get(row).map(|x| &*x.content)
    }

    pub fn get_row_render(&self, row: usize, width: usize) -> Option<&str> {
        self.row.get(self.row_off + row).map(|row| {
            let start = self.col_off;
            let end = self.col_off + width;
            &row.render[get_byte_range_from_char_range(&row.render, start, end)]
        })
    }

    pub fn remove_line(&mut self, line: usize) -> String {
        self.row.remove(line).content
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

    fn scroll_to_fit(&mut self, rows: u16, cols: u16) {
        let cx = (self.cur_col as isize - self.col_off as isize) as i32;
        let cy = (self.cur_line as isize - self.row_off as isize) as i32;

        self.fit_pos(cx, cy, rows, cols);
    }

    pub fn move_cursor(&mut self, c: CursorDirection) {
        use CursorDirection as C;

        match c {
            C::Up => self.cur_line = self.cur_line.saturating_sub(1),
            C::Down => {
                self.cur_line = (self.cur_line + 1).clamp(0, self.row.len().saturating_sub(1))
            }
            C::Left => self.cur_col = self.cur_col.saturating_sub(1),
            C::Right => self.cur_col += 1,
        }

        self.cur_col = self.cur_col.clamp(0, self.row_len(self.cur_line));
    }

    /// where to place the cursor (rows x cols coordinates)
    /// returns (y, x)
    pub fn cursor(&mut self, rows: u16, cols: u16) -> (u16, u16) {
        self.scroll_to_fit(rows, cols);
        (
            (self.cur_line - self.row_off).try_into().unwrap(),
            self.cx_to_rendered((self.cur_col - self.col_off).try_into().unwrap()),
        )
    }

    /// returns (line, col)
    pub fn position(&self) -> Location {
        Location::new(self.cur_line, self.cur_col)
    }

    /// returns (line, col)
    pub fn set_position(&mut self, line: usize, col: usize) {
        self.cur_line = line.clamp(0, self.row.len());
        self.cur_col = col.clamp(0, self.row_len(self.cur_line));
    }

    pub fn insert_char(&mut self, ch: char) {
        self.dirty = true;
        if self.row.is_empty() {
            self.row.push(Row::new(String::with_capacity(1)));
        }
        let row = &mut self.row[self.cur_line];

        row.insert_char(ch, self.cur_col);

        self.move_cursor(CursorDirection::Right);
    }

    pub fn delete_char(&mut self) {
        if self.row.is_empty() || (self.cur_line == 0 && self.cur_col == 0) {
            return;
        }
        self.dirty = true;
        let row = &mut self.row[self.cur_line];
        if self.cur_col == 0 && self.cur_line > 0 {
            let removed = self.row.remove(self.cur_line);

            self.cur_col = self.row[self.cur_line - 1].content_len();
            self.row[self.cur_line - 1].append_row(removed);
            self.cur_line -= 1;
            return;
        }
        row.delete_char(self.cur_col);
        self.move_cursor(CursorDirection::Left)
    }

    pub fn add_newline(&mut self) {
        if self.row.is_empty() {
            self.row.push(Row::new(String::new()));
        }
        self.dirty = true;
        let row = &mut self.row[self.cur_line];
        let next = row.split(self.cur_col);
        self.row.insert(self.cur_line + 1, next);

        self.cur_line += 1;
        self.cur_col = 0;
    }

    pub(crate) fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    pub fn num_lines(&self) -> usize {
        self.row.len()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn set_path(&mut self, path: String) {
        self.path = Some(path);
    }

    pub fn set_name(&mut self, name: String) {
        self.name = name;
    }

    /// behaves like `nvim_buf_get_text`
    /// lines inclusive, columns exclusive
    pub fn get_range<'a>(&'a self, start: Location, end: Location) -> RangeIter<'a> {
        RangeIter {
            buf: self,
            start,
            end,
        }
    }

    /// lines inclusive, columns exclusive
    // TODO: do we want to return the deleted text?
    pub fn delete_range(&mut self, start: Location, end: Location) {
        assert!(start < end);
        let range = if start.line() == end.line() {
            let row = &mut self.row[start.line()];
            dbg!(&row);
            row.content.drain(get_byte_range_from_char_range(
                &row.content,
                start.col(),
                end.col(),
            ));
            row.recompute_rendered();
            0..0
        } else {
            let mut end_row = self.row.remove(end.line()).content;
            let row = &mut self.row[start.line()];
            row.content
                .drain(char_idx_to_byte_idx(&row.content, start.col()).unwrap_or(0)..);

            end_row
                .drain(0..dbg!(char_idx_to_byte_idx(&end_row, end.col()).unwrap_or(end_row.len())));
            row.content.push_str(&end_row);

            row.recompute_rendered();
            dbg!(start, end);

            if end.line() == start.line() + 1 {
                return;
            }

            start.line() + 1..end.line() - 1
        };
        self.row.drain(range);
    }

    /// lines inclusive, columns exclusive
    pub fn set_range<I, S>(&mut self, start: Location, end: Location, _replacement: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        todo!("set range {start:?} {end:?}")
    }
}

pub struct RangeIter<'a> {
    buf: &'a Buffer,
    start: Location,
    end: Location,
}

impl<'a> Iterator for RangeIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.start.line() > self.end.line() {
            return None;
        }
        if self.start.line() == self.end.line() {
            let ret = self.buf.get_row(self.start.line())?;
            let range = get_byte_range_from_char_range(ret, self.start.col(), self.end.col());
            let ret = &ret[range];
            self.start = Location::new(self.start.line() + 1, 0);
            return Some(ret);
        }

        let ret = self.buf.get_row(self.start.line())?;
        let idx = char_idx_to_byte_idx(ret, self.start.col()).unwrap_or(0);
        self.start = Location::new(self.start.line() + 1, 0);

        Some(&ret[idx..])
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
                path: Some(name.clone()),
                dirty: false,
                name,
                row: vec![Row::new("hello".repeat(200))],
                row_off: 0,
                col_off: 0,
                cur_line: 0,
                cur_col: 0,
            }
        );
    }

    fn print_cursor(buf: &mut Buffer, rows: u16, cols: u16) {
        let (cy, cx) = buf.cursor(rows, cols);
        if let Some(row) = buf.get_row_render(cy.into(), cols.into()) {
            let idx = cx.into();
            let idx = row
                .char_indices()
                .nth(idx)
                .map(|x| x.0)
                .unwrap_or(row.len());
            let (before, after) = row.split_at(idx);
            eprintln!("{before}│{after}");
        }
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
            buf.move_cursor(action);
            print_cursor(buf, rows, cols);
            assert_eq!(buf.position(), pos.into(), "position: #{i} {action:?}");
            assert_eq!(buf.cursor(rows, cols), cursor, "cursor: #{i} {action:?}");
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
                (C::Right, (1, 3), (1, 3)),
                (C::Right, (1, 3), (1, 3)),
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
            buf.move_cursor(C::Right);
        }
        assert_eq!(buf.position(), (0, 11).into());
        assert_eq!(buf.cursor(rows, cols), (0, 11));

        enact(&mut buf, rows, cols, &[(C::Down, (1, 6), (1, 6))]);
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
            buf.move_cursor(C::Right);
        }
        assert_eq!(buf.position(), (0, 11).into());
        assert_eq!(buf.cursor(rows, cols), (0, 7));

        enact(&mut buf, rows, cols, &[(C::Down, (1, 6), (1, 6))]);
    }

    #[test]
    fn buffer_to_string() {
        let buf = new_buf(
            "
            this
            ",
        );
        assert_eq!(buf.save(), "this\n");
    }

    #[test]
    fn insert_char() {
        let mut buf = new_buf("this");
        assert_eq!(buf.position(), (0, 0).into());
        buf.insert_char('a');
        assert_eq!(buf.save(), "athis\n");
    }

    #[test]
    fn append_char() {
        let rows = 24;
        let cols = 80;
        let mut buf = new_buf("this");
        enact(
            &mut buf,
            rows,
            cols,
            &[
                (C::Right, (0, 1), (0, 1)),
                (C::Right, (0, 2), (0, 2)),
                (C::Right, (0, 3), (0, 3)),
                (C::Right, (0, 4), (0, 4)),
            ],
        );
        buf.insert_char('a');
        assert_eq!(buf.save(), "thisa\n");
    }

    #[test]
    fn append_char_scroll() {
        let rows = 24;
        let cols = 3;
        let mut buf = new_buf("this");
        enact(
            &mut buf,
            rows,
            cols,
            &[
                (C::Right, (0, 1), (0, 1)),
                (C::Right, (0, 2), (0, 2)),
                (C::Right, (0, 3), (0, 2)),
                (C::Right, (0, 4), (0, 3)),
            ],
        );
        buf.insert_char('a');
        assert_eq!(buf.save(), "thisa\n");
        assert_eq!(buf.position(), (0, 5).into());
    }

    #[test]
    fn remove_char_at_start_of_line() {
        let mut buf = new_buf("this");
        buf.delete_char();
        assert_eq!(buf.save(), "this\n");
        assert_eq!(buf.position(), (0, 0).into());
    }

    #[test]
    fn remove_char() {
        let rows = 24;
        let cols = 80;
        let mut buf = new_buf("this");
        enact(
            &mut buf,
            rows,
            cols,
            &[(C::Right, (0, 1), (0, 1)), (C::Right, (0, 2), (0, 2))],
        );
        buf.delete_char();
        assert_eq!(buf.save(), "tis\n");
        assert_eq!(buf.position(), (0, 1).into());
    }

    #[test]
    fn remove_linebreak() {
        let rows = 24;
        let cols = 80;
        let mut buf = new_buf("foo\nbar");
        enact(&mut buf, rows, cols, &[(C::Down, (1, 0), (1, 0))]);
        buf.delete_char();
        print_cursor(&mut buf, rows, cols);
        assert_eq!(buf.save(), "foobar\n");
        assert_eq!(buf.position(), (0, 3).into());
    }

    #[test]
    fn add_newline() {
        let rows = 24;
        let cols = 80;
        let mut buf = new_buf("this");
        enact(
            &mut buf,
            rows,
            cols,
            &[(C::Right, (0, 1), (0, 1)), (C::Right, (0, 2), (0, 2))],
        );
        buf.add_newline();
        assert_eq!(buf.save(), "th\nis\n");
        assert_eq!(buf.position(), (1, 0).into());
    }

    #[test]
    fn insert_tab() {
        let rows = 24;
        let cols = 80;
        let mut buf = new_buf("this");
        buf.insert_char('\t');
        assert_eq!(buf.get_row_render(0, cols.into()), Some("    this"));
        enact(&mut buf, rows, cols, &[(C::Right, (0, 2), (0, 5))]);
        buf.insert_char('\t');
        assert_eq!(buf.save(), "\tt\this\n");
        assert_eq!(buf.get_row_render(0, cols.into()), Some("    t    his"));
    }

    #[test]
    fn edit_empty_file() {
        let mut buf = new_buf("");
        assert!(!buf.is_dirty());
        buf.insert_char('h');
        assert!(buf.is_dirty());
        assert_eq!(buf.save(), "h\n");
    }
    #[test]
    fn delete_in_empty_file() {
        let mut buf = new_buf("");
        assert!(!buf.is_dirty());
        buf.delete_char();
        assert!(!buf.is_dirty());
        buf.scrub();
        assert!(!buf.is_dirty());
        assert_eq!(buf.save(), "");
    }

    #[test]
    fn move_cursor_with_tabs() {
        let rows = 24;
        let cols = 80;
        let mut buf = new_buf(
            "
            int main() {
            	return 0;
            }
            ",
        );
        for i in 0..13 {
            buf.move_cursor(C::Right);
            print_cursor(&mut buf, rows, cols);
            assert_eq!(buf.position(), (0, (i + 1).min(12)).into());
        }
        enact(&mut buf, rows, cols, &[(C::Down, (1, 10), (1, 13))]);
    }

    #[test]
    fn render_special() {
        let buf = new_buf("\x1b");
        assert_eq!(buf.get_row_render(0, 80), Some("X1b"));
    }

    #[test]
    fn num_lines() {
        let buf = new_buf("");
        assert_eq!(buf.num_lines(), 0);
        let buf = new_buf("foo");
        assert_eq!(buf.num_lines(), 1);
        let buf = new_buf("foo\nbar");
        assert_eq!(buf.num_lines(), 2);
    }

    #[test]
    fn default() {
        let buf = Buffer::default();
        assert!(!buf.dirty);
        assert!(buf.is_empty());
        assert_eq!(buf.position(), (0, 0).into());
        assert_eq!(buf.num_lines(), 0);
        assert!(buf.path().is_none());
    }

    #[test]
    fn name() {
        let buf = Buffer::read("name".to_string(), "");
        assert_eq!(buf.name(), "name");
    }

    macro_rules! get_range_tests {
        (
            $(
                $name:ident: $buf:expr, $start:expr, $end:expr, $expected:expr
            )*
        ) =>{

            $(

                #[test]
                fn $name() {
                    let buffer = new_buf($buf);
                    assert_eq!(
                        buffer
                            .get_range($start.into(), $end.into())
                            .collect::<Vec<_>>(),
                        $expected
                    )
                }
            )*
        };
    }

    macro_rules! delete_range_tests {
        (
            $(
                $name:ident: $buf:expr, $start:expr, $end:expr, $expected:expr
            )*
        ) =>{

            $(

                #[test]
                fn $name() {
                    let mut buffer = new_buf($buf);
                    buffer.delete_range($start.into(), $end.into());
                    assert_eq!(
                        buffer.save(),
                        $expected
                    )
                }
            )*
        };
    }

    get_range_tests! {
        get_full_range: "hello\n\nworld", (0,0), (2,5), ["hello", "", "world"]
        get_almost_full_range: "hello\n\nworld", (0,0), (2,4), ["hello", "", "worl"]
        get_empty_range: "hello\n\nworld", (0,0), (0,0), [""]
        get_on_one_line: "hello\n\nworld", (0,1), (0,3), ["el"]
        get_invalid_range: "hello\n\nworld", (0, 1), (0, 100), ["ello"]
    }

    delete_range_tests! {
        delete_in_single_line: "hello world", (0, 2), (0, 6), "heworld\n"
        delete_in_two_lines: "hello\n world", (0, 2), (1, 1), "heworld\n"
        delete_range_crash: ".\n\nuse anyhow::anyhow;", (2, 4), (2, 10), ".\n\nuse ::anyhow;\n"
    }
}
