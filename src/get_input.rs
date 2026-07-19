use std::io::Read as _;

use tinyvec::ArrayVec;

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

pub struct GetChar<Src> {
    seq: SeqMode,
    src: Src,
}

impl<Src> GetChar<Src>
where
    Src: GetCharSource,
{
    pub fn new(src: Src) -> Self {
        Self {
            seq: SeqMode::Waiting,
            src,
        }
    }

    fn map(x: u8) -> Input {
        match x {
            127 => Input::Backspace,
            b'\r' | b'\n' => Input::Enter,
            _ => Input::Char(x),
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
            return Some(Self::map(ret));
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

        Some(Self::map(ch))
    }
}

#[cfg(test)]
mod tests {
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

    fn poll(getch: &mut GetChar<MockInput>, n: usize) -> Vec<Input> {
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
                Input::Char(b'h'),
                Input::Char(b'e'),
                Input::Char(b'l'),
                Input::Char(b'l'),
                Input::Char(b'o'),
                Input::Char(b' '),
                Input::Char(b'w'),
                Input::Char(b'o'),
                Input::Char(b'r'),
                Input::Char(b'l'),
                Input::Char(b'd'),
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
                Input::Char(b'h'),
                Input::Char(b'e'),
                Input::Char(b'l'),
                Input::Char(b'l'),
                Input::Char(b'o'),
                Input::Arrow(CursorDirection::Left)
            ]
        );
    }

    #[test]
    fn unrecognized_control_sequence() {
        let i = MockInput::new().chars("\x1b[,").pause();
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(
            x,
            vec![Input::Char(b'\x1b'), Input::Char(b'['), Input::Char(b',')]
        );
    }

    #[test]
    fn long_unrecognized() {
        let i = MockInput::new().chars("\x1b[,1234567890").pause();
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(
            x,
            vec![
                Input::Char(b'\x1b'),
                Input::Char(b'['),
                Input::Char(b','),
                Input::Char(b'1'),
                Input::Char(b'2'),
                Input::Char(b'3'),
                Input::Char(b'4'),
                Input::Char(b'5'),
                Input::Char(b'6'),
                Input::Char(b'7'),
                Input::Char(b'8'),
                Input::Char(b'9'),
                Input::Char(b'0'),
            ]
        );
    }

    #[test]
    fn backspace() {
        let i = MockInput::new().chars("h\x7f");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, vec![Input::Char(b'h'), Input::Backspace]);
    }

    #[test]
    fn newline() {
        let i = MockInput::new().chars("h\n");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, vec![Input::Char(b'h'), Input::Enter]);
    }

    #[test]
    fn escape() {
        let i = MockInput::new().chars("\x1b");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, vec![Input::Escape]);
    }

    #[test]
    fn arrow_keys() {
        let i = MockInput::new().chars("\x1b[A\x1b[C\x1b[B\x1b[D");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(
            x,
            vec![
                Input::Arrow(CursorDirection::Up),
                Input::Arrow(CursorDirection::Right),
                Input::Arrow(CursorDirection::Down),
                Input::Arrow(CursorDirection::Left),
            ]
        );
    }

    #[test]
    fn page_up() {
        let i = MockInput::new().chars("\x1b[5~\x1b[6~");
        let mut gc = GetChar::new(i);

        let x = poll(&mut gc, 20);
        assert_eq!(x, vec![Input::PageUp, Input::PageDown]);
    }
}
