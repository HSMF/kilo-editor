use std::ops::ControlFlow;

use log::{debug, warn};
use tinyvec::{TinyVec, tiny_vec};

use crate::{CursorDirection, Input, buffer::Buffer, ctrl_key, trie::Trie};

#[derive(PartialEq, Eq, Clone, Copy, Debug, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
}

#[derive(Debug, PartialEq)]
pub struct RegisterFile {
    unnamed: String,
}

impl Mode {
    pub fn str(&self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
        }
    }
}

pub struct VimState {
    mode: Mode,
    quit: bool,
    registers: RegisterFile,
}

type MappingFunc = dyn Fn(MapArgs);
type Keymaps = Trie<Input, Box<MappingFunc>>;

pub struct Vim {
    state: VimState,
    cur_input: TinyVec<[Input; 4]>,
    normal_keymaps: Keymaps,
    insert_keymaps: Keymaps,
    visual_keymaps: Keymaps,
}

pub struct MapArgs<'a> {
    buf: &'a mut Buffer,
    state: &'a mut VimState,
}
impl<'a> MapArgs<'a> {
    fn new(buf: &'a mut Buffer, state: &'a mut VimState) -> Self {
        Self { buf, state }
    }
}

enum LookupKeymap<'a> {
    Match(&'a MappingFunc),
    Continue(()),
    NoMatch,
}

impl Vim {
    pub fn bare() -> Self {
        Self {
            state: VimState::new(),
            cur_input: tiny_vec!(),
            normal_keymaps: Keymaps::new(),
            insert_keymaps: Keymaps::new(),
            visual_keymaps: Keymaps::new(),
        }
    }

    pub fn new() -> Self {
        let mut ret = Self::bare();
        ret.configure_normal_mode();
        ret.configure_insert_mode();
        ret
    }

    fn handle_key<'a>(
        key: impl IntoIterator<Item = &'a Input> + 'a,
        keymaps: &'a Keymaps,
    ) -> LookupKeymap<'a> {
        match keymaps.get(key) {
            Some(crate::trie::GetResult::Subtrie(..)) => LookupKeymap::Continue(()),
            Some(crate::trie::GetResult::Value(v)) => LookupKeymap::Match(v),
            None => LookupKeymap::NoMatch,
        }
    }

    pub fn add_keymap<F: Fn(MapArgs) + 'static>(
        &mut self,
        mode: Mode,
        mapping: impl IntoIterator<Item = Input>,
        action: F,
    ) {
        macro_rules! insert {
            ($self:expr, $field:ident) => {{
                $self.$field.insert(mapping, Box::new(action));
            }};
        }
        match mode {
            Mode::Normal => insert!(self, normal_keymaps),
            Mode::Insert => insert!(self, insert_keymaps),
            Mode::Visual => insert!(self, visual_keymaps),
        }
    }

    pub fn fallback_insert(&mut self, buf: &mut Buffer, ch: Input) {
        match ch {
            Input::Char(ch) => buf.insert_char(ch as char),
            _ => warn!("unhandled char {ch:?}"),
        }
    }

    pub fn handle_input(&mut self, buf: &mut Buffer, ch: Input) -> ControlFlow<()> {
        macro_rules! handle_mode {
            ($self:expr, $keymaps:ident) => {
                handle_mode!($self, $keymaps, fallback = {})
            };
            ($self:expr, $keymaps:ident, fallback = $fallback:expr) => {{
                $self.cur_input.push(ch);
                match Self::handle_key($self.cur_input.iter(), &$self.$keymaps) {
                    LookupKeymap::Match(fun) => {
                        fun(MapArgs::new(buf, &mut self.state));
                        $self.cur_input.clear();
                    }
                    LookupKeymap::Continue(..) => {}
                    LookupKeymap::NoMatch => {
                        $fallback;
                        $self.cur_input.clear();
                    }
                }
            }};
        }
        match self.state.mode {
            Mode::Normal => handle_mode!(self, normal_keymaps),
            Mode::Insert => {
                handle_mode!(
                    self,
                    insert_keymaps,
                    fallback = self.fallback_insert(buf, ch)
                )
            }

            Mode::Visual => todo!(),
        }
        if self.state.quit {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }

    pub fn mode(&self) -> Mode {
        self.state.mode
    }
}

impl Vim {
    fn configure_normal_mode(&mut self) {
        let mode = Mode::Normal;
        use CursorDirection as C;
        use Input as I;
        self.add_keymap(mode, [I::Escape], |a| a.state.mode = Mode::Normal);
        self.add_keymap(mode, [I::Char(ctrl_key(b'w'))], |a| a.state.quit = true);
        self.add_keymap(mode, [I::Char(ctrl_key(b'u'))], |a| {
            for _ in 0..12 {
                a.buf.move_cursor(C::Up)
            }
        });
        self.add_keymap(mode, [I::Char(ctrl_key(b'd'))], |a| {
            for _ in 0..12 {
                a.buf.move_cursor(C::Down)
            }
        });
        self.add_keymap(mode, [I::Char(b'h')], |a| a.buf.move_cursor(C::Left));
        self.add_keymap(mode, [I::Char(b'j')], |a| a.buf.move_cursor(C::Down));
        self.add_keymap(mode, [I::Char(b'k')], |a| a.buf.move_cursor(C::Up));
        self.add_keymap(mode, [I::Char(b'l')], |a| a.buf.move_cursor(C::Right));
        self.add_keymap(mode, [I::Char(b'i')], |a| a.state.mode = Mode::Insert);
        self.add_keymap(mode, [I::Char(b'a')], |a| {
            let (line, col) = a.buf.position();
            a.buf.set_position(line, col + 1);
            a.state.mode = Mode::Insert
        });
        self.add_keymap(mode, [I::Char(b'o')], |a| {
            let (line, _) = a.buf.position();
            if let Some(row) = a.buf.get_row(line) {
                a.buf.set_position(line, row.chars().count());
            }
            a.buf.add_newline();
            a.state.mode = Mode::Insert
        });
        self.add_keymap(mode, [I::Char(b'0')], |a| {
            let (line, _) = a.buf.position();
            a.buf.set_position(line, 0);
        });
        self.add_keymap(mode, [I::Char(b'$')], |a| {
            let (line, _) = a.buf.position();
            let last = a.buf.get_row(line).unwrap_or("").len();
            a.buf.set_position(line, last);
        });
        self.add_keymap(mode, [I::Char(b'y'), I::Char(b'y')], |a| {
            let (line, _) = a.buf.position();
            let line = a.buf.get_row(line).unwrap_or("").to_owned();
            a.state.registers.set_register('"', line);
        });
        self.add_keymap(mode, [I::Char(b'd'), I::Char(b'd')], |a| {
            let (line, _) = a.buf.position();
            let content = a.buf.remove_line(line);
            a.state.registers.set_register('"', content);
        });
        self.add_keymap(mode, [I::Char(b'p')], |a| {
            let (line, ..) = a.buf.position();
            let content = a.buf.get_row(line).unwrap_or("");
            a.buf.set_position(line, content.len());
            a.buf.add_newline();
            for ch in a.state.registers.get_register('"').chars() {
                a.buf.insert_char(ch);
            }
        });
        self.add_keymap(mode, [I::Char(b'w')], |a| {
            let (line, col) = a.buf.position();
            let Some(row) = a.buf.get_row(line) else {
                return;
            };
            #[allow(clippy::skip_while_next)]
            let new_col = row
                .chars()
                .enumerate()
                .skip(col)
                .skip_while(|(_, ch)| ch.is_alphanumeric())
                .skip_while(|(_, ch)| !ch.is_alphanumeric())
                .next()
                .map(|x| x.0)
                .unwrap_or_else(|| row.chars().count());
            debug!("next word starts at {new_col}");
            a.buf.set_position(line, new_col);
        });
        self.add_keymap(mode, [I::Char(b'd'), Input::Char(b'w')], |_| {
            debug!("delete word");
        });
        // TODO: once command mode is supported, drop this
        self.add_keymap(mode, [I::Char(b':'), I::Char(b'q')], |a| {
            a.state.quit = true;
        });
        // TODO: once command mode is supported, drop this
        self.add_keymap(mode, [I::Char(b':'), I::Char(b'w')], |a| {
            let Some(path) = a.buf.path() else {
                return;
            };
            let path = path.to_owned();
            a.buf.scrub();
            let s = a.buf.save();
            std::fs::write(path, &s).expect("cant write");
        });
        self.configure_arrow_keys(mode);
    }

    fn configure_insert_mode(&mut self) {
        let mode = Mode::Insert;
        use Input as I;
        self.add_keymap(mode, [I::Escape], |a| a.state.mode = Mode::Normal);
        self.configure_arrow_keys(mode);
        self.add_keymap(mode, [I::Backspace], |a| a.buf.delete_char());
        self.add_keymap(mode, [I::Enter], |a| a.buf.add_newline());
    }

    fn configure_arrow_keys(&mut self, mode: Mode) {
        use CursorDirection as C;
        use Input as I;
        for dir in [C::Left, C::Right, C::Up, C::Down] {
            self.add_keymap(mode, [I::Arrow(dir)], move |a| a.buf.move_cursor(dir));
        }
    }
}

impl VimState {
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            quit: false,
            registers: RegisterFile::new(),
        }
    }
}

impl RegisterFile {
    pub fn get_register(&self, name: char) -> &str {
        assert_eq!(name, '"', "currently only unnamed (\") is supported");
        &self.unnamed
    }

    pub fn set_register(&mut self, name: char, s: String) {
        assert_eq!(name, '"', "currently only unnamed (\") is supported");
        self.unnamed = s;
    }

    fn new() -> Self {
        Self {
            unnamed: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::*;

    #[test]
    fn handle_key() {
        let mut v = Vim::bare();
        let mut buf = Buffer::new();
        let x = Rc::new(Cell::new(0));
        let y = Rc::clone(&x);
        v.add_keymap(
            Mode::Normal,
            [Input::Char(b'a'), Input::Char(b'b')],
            move |_| x.set(1),
        );

        assert_eq!(y.get(), 0);

        let _ = v.handle_input(&mut buf, Input::Char(b'a'));
        assert_eq!(y.get(), 0);

        let _ = v.handle_input(&mut buf, Input::Char(b'b'));

        assert_eq!(y.get(), 1);
    }
}
