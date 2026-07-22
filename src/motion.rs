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

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum CharClass {
    Word,
    Symbol,
    Space,
}

trait Classify {
    fn classify(&self, ch: char) -> CharClass;
}

/// `word`
struct WordClassify;
impl Classify for WordClassify {
    fn classify(&self, ch: char) -> CharClass {
        if ch.is_alphanumeric() {
            CharClass::Word
        } else if ch.is_whitespace() {
            CharClass::Space
        } else {
            CharClass::Symbol
        }
    }
}

/// `WORD`
struct BigWordClassify;
impl Classify for BigWordClassify {
    fn classify(&self, ch: char) -> CharClass {
        if ch.is_whitespace() {
            CharClass::Space
        } else {
            CharClass::Symbol
        }
    }
}

/// `w`
struct WordMotion<C> {
    classify: C,
}

impl<C> WordMotion<C> {
    pub fn new(classify: C) -> Self {
        Self { classify }
    }
}

impl<C> Motion for WordMotion<C>
where
    C: Classify,
{
    fn next(&self, buf: &Buffer) -> Option<Location> {
        let (line, col) = buf.position().destruct();
        let cur_line = buf.get_row(line)?;
        let (idx, first_ch) = cur_line.char_indices().nth(col).unwrap_or((0, ' '));

        // skip the current word
        let rest = &cur_line[idx..];
        let mut col = col;
        let first_class = self.classify.classify(first_ch);
        let (n, first_after) = rest
            .char_indices()
            .find_idx(|ch| self.classify.classify(ch.1) != first_class);
        let first_after = first_after.map(|x| x.0).unwrap_or(rest.len());
        col += n;

        // skip whitespace, but only one line, idk... vim is weird
        let rest = &rest[first_after..];

        let (n, first_after) = rest
            .char_indices()
            .find_idx(|ch| self.classify.classify(ch.1) != CharClass::Space);
        if first_after.is_some() {
            return Some(Location::new(line, col + n));
        }

        let Some(rest) = buf.get_row(line + 1) else {
            return Some(Location::new(line, col + n));
        };

        let (n, _) = rest
            .char_indices()
            .find_idx(|ch| self.classify.classify(ch.1) != CharClass::Space);
        Some(Location::new(line + 1, n))
    }
}

struct BackMotion<C> {
    classify: C,
}

impl<C> BackMotion<C> {
    pub fn new(classify: C) -> Self {
        Self { classify }
    }
}

impl<C> Motion for BackMotion<C>
where
    C: Classify,
{
    fn next(&self, buf: &Buffer) -> Option<Location> {
        let (line, col) = buf.position().destruct();
        let Some(cur_line) = buf.get_row(line) else {
            return Some(buf.position());
        };
        let (idx, first_ch) = cur_line.char_indices().nth(col).unwrap_or((0, ' '));
        let first_class = self.classify.classify(first_ch);

        let start = &cur_line[..idx];

        // unfortunate naming
        let skip_in_class = |s: &str, class: CharClass| {
            let (n, first_after) = s
                .char_indices()
                .rev()
                .find_idx(|ch| self.classify.classify(ch.1) != class);
            (n, first_after.map(|x| x.0 + x.1.len_utf8()))
        };

        let (n, first_after) = skip_in_class(start, first_class);

        let col = col - n;
        if first_after.is_some() && n != 0 {
            return Some(Location::new(line, col));
        }

        let l = first_after.unwrap_or(0);
        let start = &start[..l];
        let (n, first_after) = skip_in_class(start, CharClass::Space);
        let col = col - n;

        let mut line = line;
        let mut col = col;
        let l = first_after.unwrap_or(0);
        let start = &start[..l];

        let mut start = start;
        if col == 0 && line > 0 {
            line -= 1;
            let cur_line = buf.get_row(line).expect("previous line exists");
            col = cur_line.chars().count();

            let (n, first_after) = skip_in_class(cur_line, CharClass::Space);
            start = &cur_line[..first_after.unwrap_or(0)];
            col -= n;
        }

        let last_class = start
            .chars()
            .last()
            .map(|x| self.classify.classify(x))
            .unwrap_or(CharClass::Space);

        let (n, _) = skip_in_class(start, last_class);
        let col = col - n;

        Some(Location::new(line, col))
    }
}

macro_rules! motion_with_word_classification {
    (
        $(
        $(#[$meta:meta])*
        pub struct $name:ident($motion:ident, $classifier:ident);
        )*
        ) => {
        $(
            $(#[$meta])*
            pub struct $name($motion<$classifier>);

            impl $name {
                pub fn new() -> Self {
                    Self($motion::new($classifier))
                }
            }

            impl Default for $name {
                fn default() -> Self {
                    Self::new()
                }
            }

            impl Motion for $name {
                fn next(&self, buf: &Buffer) -> Option<Location> {
                    self.0.next(buf)
                }
            }
        )*
    };
}

motion_with_word_classification! {
    /// `w`
    pub struct Word(WordMotion, WordClassify);
    /// `W`
    pub struct BigWord(WordMotion, BigWordClassify);
    /// `b`
    pub struct Back(BackMotion, WordClassify);
    /// `B`
    pub struct BigBack(BackMotion, BigWordClassify);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(s: &str) -> Buffer {
        Buffer::read("t.rs".to_owned(), s)
    }

    macro_rules! motion_test {
        ($(
                $name:ident: $motion:ident, $buf:expr, $start_loc:expr, $end_loc:expr ;
        )*) => {
            $(
                #[test]
                fn $name() {
                    let mut buf = buffer($buf);
                    let motion = $motion::new();
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
        word_path1: Word, "core::mem::swap", (0,0), (0,4);
        word_path2: Word, "core::mem::swap", (0,4), (0,6);
        word_path3: Word, "core::mem::swap", (0,6), (0,9);
        word_path4: Word, "core::mem::swap", (0,9), (0,11);
        word_path5: Word, "core::mem::swap", (0,11), (0,15);
        // word_newline: Word, "hello\nworld", (0,0), (1,0);
        word_empty_lines: Word, "\n\nhello", (0,0), (1,0);
        word_empty_lines2: Word, "\n\nhello", (1,0), (2,0);
        word_weird: Word, "use anyhow::anyhow;\nuse log::LevelFilter;", (0, 18), (1,0);

        big_word_start: BigWord, "hello world", (0,0), (0,6);
        big_word_end_of_buf: BigWord, "hello world", (0,6), (0,11);
        big_word_middle: BigWord, "hello world", (0,3), (0,6);
        big_word_non_word: BigWord, "core::mem::swap", (0,4), (0,15);
        big_word_path1: BigWord, "core::mem::swap", (0,0), (0,15);
        big_word_path2: BigWord, "core::mem::swap", (0,4), (0,15);
        big_word_path3: BigWord, "core::mem::swap", (0,6), (0,15);
        big_word_path4: BigWord, "core::mem::swap", (0,11), (0,15);
        big_word_empty_lines: BigWord, "\n\nhello", (0,0), (1,0);
        big_word_empty_lines2: BigWord, "\n\nhello", (1,0), (2,0);
        big_word_weird: BigWord, "use anyhow::anyhow;\nuse log::LevelFilter;", (0, 18), (1,0);

        back_empty: Back, "", (0,0), (0,0);
        back_start: Back, "hello world", (0,0), (0,0);
        back_end1: Back, "hello world", (0,10), (0,6);
        back_end2: Back, "hello world", (0,6), (0,0);
        back_path1: Back, "core::mem::swap", (0,14), (0,11);
        back_path2: Back, "core::mem::swap", (0,11), (0,9);
        back_path3: Back, "core::mem::swap", (0,9), (0,6);
        back_path4: Back, "core::mem::swap", (0,6), (0,4);
        back_path5: Back, "core::mem::swap", (0,4), (0,0);
        back_newline: Back, "hello\nworld", (1,0), (0,0);
        back_newline_trail: Back, "hello  \nworld", (1,0), (0,0);
    }

    #[test]
    fn requirement_for_classification() {
        assert_eq!(WordClassify.classify('\n'), CharClass::Space);
        assert_eq!(BigWordClassify.classify('\n'), CharClass::Space);
    }
}
