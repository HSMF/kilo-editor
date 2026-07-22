use crate::{buffer::Buffer, location::Location};

pub trait Motion {
    fn next(&self, buf: &Buffer) -> Option<Location>;
}

trait FindIdxExt: Iterator {
    fn find_idx<F>(self, f: F) -> (usize, Option<Self::Item>)
    where
        F: FnMut(&Self::Item) -> bool;
}

impl<I> FindIdxExt for I
where
    I: Iterator,
{
    fn find_idx<F>(self, mut f: F) -> (usize, Option<Self::Item>)
    where
        F: FnMut(&Self::Item) -> bool,
    {
        let mut idx = 0;
        for item in self {
            if f(&item) {
                return (idx, Some(item));
            }
            idx += 1;
        }
        (idx, None)
    }
}

pub struct Word;

impl Word {
    // fn inside(&self, buf: &Buffer) -> (Location, Location) {
    //     let (line, col) = buf.position().destruct();
    //     let cur_line = buf.get_row(line).unwrap_or("");
    //     let Some(cur_char) = cur_line.char_indices().nth(col) else {
    //         return (Location::default(), Location::default());
    //     };
    //
    //     todo!()
    // }
    fn is_inside(&self, ch: char) -> bool {
        ch.is_alphanumeric()
    }
}

impl Motion for Word {
    fn next(&self, buf: &Buffer) -> Option<Location> {
        let (line, col) = buf.position().destruct();
        let cur_line = buf.get_row(line)?;
        let idx = cur_line.char_indices().map(|x| x.0).nth(col).unwrap_or(0);

        // skip the current word
        let rest = &cur_line[idx..];
        let mut col = col;
        let (n, first_after) = rest.char_indices().find_idx(|ch| !self.is_inside(ch.1));
        let first_after = first_after.map(|x| x.0).unwrap_or(rest.len());
        col += n;

        // skip whitespace, but only one line, idk... vim is weird
        let rest = &rest[first_after..];

        let (n, first_after) = rest.char_indices().find_idx(|ch| self.is_inside(ch.1));
        if first_after.is_some() {
            return Some(Location::new(line, col + n));
        }

        let Some(rest) = buf.get_row(line + 1) else {
            return Some(Location::new(line, col + n));
        };

        let (n, _) = rest.char_indices().find_idx(|ch| self.is_inside(ch.1));
        Some(Location::new(line + 1, n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(s: &str) -> Buffer {
        Buffer::read("t.rs".to_owned(), s)
    }

    macro_rules! motion_test {
        ($(
                $name:ident: $motion:expr, $buf:expr, $start_loc:expr, $end_loc:expr ;
        )*) => {
            $(
                #[test]
                fn $name() {
                    let mut buf = buffer($buf);
                    let motion = $motion;
                    let start = $start_loc;
                    buf.set_position(start.0, start.1);

                    let moved_to = motion.next(&buf);
                    assert_eq!(moved_to, Some($end_loc.into()), "{} {:?} starting at {:?}", stringify!($motion), $buf, start)
                }
            )*
        };
    }

    motion_test! {
        word_start: Word, "hello world", (0,0), (0,6);
        word_end_of_buf: Word, "hello world", (0,6), (0,11);
        word_middle: Word, "hello world", (0,3), (0,6);
        word_non_word: Word, "core::mem::swap", (0,4), (0,6);
        // word_newline: Word, "hello\nworld", (0,0), (1,0);
        word_empty_lines: Word, "\n\nhello", (0,0), (1,0);
        word_empty_lines2: Word, "\n\nhello", (1,0), (2,0);
        word_weird: Word, "use anyhow::anyhow;\nuse log::LevelFilter;", (0, 12), (1,0);
    }
}
