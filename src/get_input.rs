use core::str;
use std::io::Read as _;

use log::error;
use tinyvec::{ArrayVec, array_vec};

use crate::{CursorDirection, Input};

pub trait GetCharSource {
    fn get_char(&mut self) -> Option<u8>;
}

pub struct StdinSource;
impl GetCharSource for StdinSource {
    fn get_char(&mut self) -> Option<u8> {
        let mut buf = [0; 1];
        let n = std::io::stdin().read(&mut buf).unwrap();
        if n == 0 {
            return None;
        }
        Some(buf[0])
    }
}

#[derive(Debug)]
enum SeqMode {
    Waiting,
    Fill(ArrayVec<[u8; 6]>),
    Drain(ArrayVec<[u8; 6]>),
}

impl SeqMode {
    pub fn reset(&mut self) {
        *self = Self::Waiting;
    }

    pub fn push(&mut self, ch: u8) {
        match self {
            SeqMode::Waiting => {
                *self = Self::Fill(tinyvec::array_vec!(_ => ch));
            }
            SeqMode::Fill(array_vec) => {
                array_vec.push(ch);
                if array_vec.len() == array_vec.capacity() {
                    let t = *array_vec;
                    *self = Self::Drain(t)
                }
            }
            SeqMode::Drain(x) => {
                if x.try_push(ch).is_some() {
                    panic!("cannot push while draining")
                }
            }
        }
    }
}

#[derive(Debug)]
struct Utf8Encoder {
    /// set by first byte, how many continuation bytes we expect
    expected_len: usize,
    buf: ArrayVec<[u8; 4]>,
}

pub struct Utf8Error;

impl Utf8Encoder {
    fn new() -> Self {
        Self {
            expected_len: 0,
            buf: array_vec!(),
        }
    }

    fn reset(&mut self) {
        self.expected_len = 0;
        self.buf.clear();
    }

    fn push(&mut self, ch: u8) -> Result<Option<char>, Utf8Error> {
        eprintln!("{ch:02x} {ch:08b}");
        match ch {
            0..=0b0111_1111 => {
                if self.expected_len == 0 {
                    Ok(Some(ch as char))
                } else {
                    Err(Utf8Error)
                }
            }
            0b1100_0000..=0b1101_1111 => {
                // two-byte codepoint
                if self.expected_len == 0 {
                    self.buf.clear();
                    self.expected_len = 1;
                    self.buf.push(ch);
                    Ok(None)
                } else {
                    Err(Utf8Error)
                }
            }
            0b1110_0000..=0b1110_1111 => {
                // three-byte codepoint
                if self.expected_len == 0 {
                    self.buf.clear();
                    self.expected_len = 2;
                    self.buf.push(ch);
                    Ok(None)
                } else {
                    Err(Utf8Error)
                }
            }
            0b1000_0000..=0b1011_1111 => {
                if self.buf.len() > self.expected_len || self.expected_len == 0 {
                    return Err(Utf8Error);
                }

                self.buf.push(ch);

                if self.buf.len() == self.expected_len + 1 {
                    let ret = str::from_utf8(&self.buf)
                        .map_err(|_| Utf8Error)?
                        .chars()
                        .next()
                        .map(Some)
                        .ok_or(Utf8Error);
                    self.reset();
                    ret
                } else {
                    Ok(None)
                }
            }
            _ => Err(Utf8Error),
        }
    }
}

pub struct GetChar<Src> {
    seq: SeqMode,
    src: Src,
    enc: Utf8Encoder,
}

impl<Src> GetChar<Src>
where
    Src: GetCharSource,
{
    pub fn new(src: Src) -> Self {
        Self {
            seq: SeqMode::Waiting,
            src,
            enc: Utf8Encoder::new(),
        }
    }

    pub fn emit_char(&mut self, ch: u8) -> Option<Input> {
        let x = ch;
        match x {
            127 => {
                self.enc.reset();
                return Some(Input::Backspace);
            }
            b'\r' | b'\n' => {
                self.enc.reset();
                return Some(Input::Enter);
            }
            _ => {}
        }

        match self.enc.push(ch) {
            Ok(Some(ch)) => Some(Input::Char(ch)),
            Ok(None) => None,
            Err(_) => {
                error!("got invalid utf-8: {:?} {ch:?}", self.enc);
                self.enc.reset();
                None
            }
        }
    }

    pub fn getch(&mut self) -> Option<Input> {
        if let SeqMode::Drain(mut v) = self.seq {
            let ret = v.remove(0);
            if v.is_empty() {
                self.seq = SeqMode::Waiting;
            } else {
                self.seq = SeqMode::Drain(v);
            }
            return self.emit_char(ret);
        }
        let Some(ch) = self.src.get_char() else {
            if let SeqMode::Fill(av) = self.seq {
                if av.as_slice() == [0x1b] {
                    self.seq = SeqMode::Waiting;
                    return Some(Input::Escape);
                }

                self.seq = SeqMode::Drain(av)
            }
            return None;
        };
        if let SeqMode::Waiting = self.seq
            && ch == 0x1b
        {
            self.seq.push(ch);
            return None;
        }

        if let SeqMode::Fill(_) = self.seq {
            self.seq.push(ch);

            if let SeqMode::Fill(t) = self.seq {
                match t.as_slice() {
                    [0x1b, b'[', b'A'] => {
                        self.seq.reset();
                        return Some(Input::Arrow(CursorDirection::Up));
                    }
                    [0x1b, b'[', b'B'] => {
                        self.seq.reset();
                        return Some(Input::Arrow(CursorDirection::Down));
                    }
                    [0x1b, b'[', b'C'] => {
                        self.seq.reset();
                        return Some(Input::Arrow(CursorDirection::Right));
                    }
                    [0x1b, b'[', b'D'] => {
                        self.seq.reset();
                        return Some(Input::Arrow(CursorDirection::Left));
                    }
                    [0x1b, b'[', d, b'~'] if b'0' <= *d && *d <= b'9' => {
                        self.seq.reset();
                        if *d == b'5' {
                            return Some(Input::PageUp);
                        } else if *d == b'6' {
                            return Some(Input::PageDown);
                        }
                    }
                    _ => {}
                }
            }

            return None;
        }

        self.emit_char(ch)
    }
}

#[cfg(test)]
mod tests {
    use Input as I;
    use Input::Char as C;
    use std::collections::VecDeque;

    use super::*;

    enum Event {
        Char(u8),
        Pause,
    }

    struct MockInput {
        events: VecDeque<Event>,
    }

    impl MockInput {
        fn new() -> Self {
            MockInput {
                events: VecDeque::new(),
            }
        }
        fn char(mut self, ch: u8) -> Self {
            self.events.push_back(Event::Char(ch));
            self
        }
        fn chars(mut self, s: &str) -> Self {
            for ch in s.bytes() {
                self.events.push_back(Event::Char(ch));
            }
            self
        }
        fn pause(mut self) -> Self {
            self.events.push_back(Event::Pause);
            self
        }
    }

    impl GetCharSource for MockInput {
        fn get_char(&mut self) -> Option<u8> {
            match self.events.pop_front() {
                None | Some(Event::Pause) => None,
                Some(Event::Char(ch)) => Some(ch),
            }
        }
    }

    fn poll(getch: &mut GetChar<MockInput>, n: usize) -> Vec<I> {
        let mut ret = vec![];
        for _ in 0..n {
            if let Some(ch) = getch.getch() {
                ret.push(ch);
            }
        }
        ret
    }

    #[test]
    fn normal_flow() {
        let i = MockInput::new()
            .chars("hello")
            .pause()
            .char(b' ')
            .chars("world");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(
            x,
            vec![
                I::Char('h'),
                I::Char('e'),
                I::Char('l'),
                I::Char('l'),
                I::Char('o'),
                I::Char(' '),
                I::Char('w'),
                I::Char('o'),
                I::Char('r'),
                I::Char('l'),
                I::Char('d'),
            ]
        );
    }

    #[test]
    fn control_sequence() {
        let i = MockInput::new().chars("hello\x1b[D");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(
            x,
            vec![
                I::Char('h'),
                I::Char('e'),
                I::Char('l'),
                I::Char('l'),
                I::Char('o'),
                I::Arrow(CursorDirection::Left)
            ]
        );
    }

    #[test]
    fn unrecognized_control_sequence() {
        let i = MockInput::new().chars("\x1b[,").pause();
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, vec![I::Char('\x1b'), I::Char('['), I::Char(',')]);
    }

    #[test]
    fn long_unrecognized() {
        let i = MockInput::new().chars("\x1b[,1234567890").pause();
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(
            x,
            vec![
                I::Char('\x1b'),
                I::Char('['),
                I::Char(','),
                I::Char('1'),
                I::Char('2'),
                I::Char('3'),
                I::Char('4'),
                I::Char('5'),
                I::Char('6'),
                I::Char('7'),
                I::Char('8'),
                I::Char('9'),
                I::Char('0'),
            ]
        );
    }

    #[test]
    fn backspace() {
        let i = MockInput::new().chars("h\x7f");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, vec![I::Char('h'), I::Backspace]);
    }

    #[test]
    fn newline() {
        let i = MockInput::new().chars("h\n");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, vec![I::Char('h'), I::Enter]);
    }

    #[test]
    fn escape() {
        let i = MockInput::new().chars("\x1b");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, vec![I::Escape]);
    }

    #[test]
    fn arrow_keys() {
        let i = MockInput::new().chars("\x1b[A\x1b[C\x1b[B\x1b[D");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(
            x,
            vec![
                I::Arrow(CursorDirection::Up),
                I::Arrow(CursorDirection::Right),
                I::Arrow(CursorDirection::Down),
                I::Arrow(CursorDirection::Left),
            ]
        );
    }

    #[test]
    fn page_up() {
        let i = MockInput::new().chars("\x1b[5~\x1b[6~");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, vec![I::PageUp, I::PageDown]);
    }

    #[test]
    fn get_utf8() {
        let i = MockInput::new().chars("äaöoüu");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, [C('ä'), C('a'), C('ö'), C('o'), C('ü'), C('u'),]);
    }

    #[test]
    fn get_utf8_with_pause() {
        let c = "ü".as_bytes();
        let i = MockInput::new().char(c[0]).pause().char(c[1]);
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, [C('ü')]);
    }

    #[test]
    fn three_byte_utf8() {
        let i = MockInput::new().chars("hi桁");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, [C('h'), C('i'), C('桁')]);
    }
}
