use std::{env, ops::ControlFlow, process::Command, str::Chars};

use log::{debug, warn};
use tinyvec::{TinyVec, tiny_vec};

use crate::{
    CursorDirection, Input,
    buffer::Buffer,
    ctrl_key,
    location::Location,
    motion::{self, Back, BigBack, BigWord, Motion, Word},
    trie::Trie,
};

#[derive(PartialEq, Eq, Clone, Copy, Debug, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    Command,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RegisterEntry {
    value: String,
    yanked_linewise: bool,
}
impl RegisterEntry {
    fn new(value: String) -> Self {
        Self {
            value,
            yanked_linewise: false,
        }
    }

    pub fn chars(&self) -> Chars<'_> {
        self.value.chars()
    }
}

#[derive(Debug, PartialEq)]
pub struct RegisterFile {
    unnamed: RegisterEntry,
}

impl Mode {
    pub fn str(&self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::Command => "COMMAND",
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
enum ModeState {
    #[default]
    Normal,
    Insert,
    Visual,
    Command {
        cmdline: String,
    },
}

impl ModeState {
    fn expect_command(&mut self) -> &mut String {
        match self {
            Self::Command { cmdline } => cmdline,
            _ => panic!(
                "expected to have state 'COMMAND' but got {}",
                Mode::from(&*self).str()
            ),
        }
    }
}

impl From<&ModeState> for Mode {
    fn from(value: &ModeState) -> Self {
        match value {
            ModeState::Normal => Self::Normal,
            ModeState::Insert => Self::Insert,
            ModeState::Visual => Self::Visual,
            ModeState::Command { .. } => Self::Command,
        }
    }
}

pub struct VimState {
    mode: ModeState,
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
    command_keymaps: Keymaps,
}

pub struct MapArgs<'a> {
    buf: &'a mut Buffer,
    state: &'a mut VimState,
}
impl<'a> MapArgs<'a> {
    fn new(buf: &'a mut Buffer, state: &'a mut VimState) -> Self {
        Self { buf, state }
    }

    fn set_mode(&mut self, mode: ModeState) {
        self.state.set_mode(mode, self.buf);
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
            command_keymaps: Keymaps::new(),
        }
    }

    pub fn new() -> Self {
        let mut ret = Self::bare();
        ret.configure_normal_mode();
        ret.configure_insert_mode();
        ret.configure_command_mode();
        ret.configure_visual_mode();
        ret
    }

    pub fn command_str(&self) -> Option<&str> {
        match &self.state.mode {
            ModeState::Command { cmdline } => Some(cmdline),
            _ => None,
        }
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
            Mode::Command => insert!(self, command_keymaps),
        }
    }

    pub fn fallback_insert(&mut self, buf: &mut Buffer, ch: Input) {
        match ch {
            Input::Char(ch) => buf.insert_char(ch),
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
        match &mut self.state.mode {
            ModeState::Normal => handle_mode!(
                self,
                normal_keymaps,
                fallback = { debug!("unknown input {:?}", self.cur_input) }
            ),
            ModeState::Insert => {
                handle_mode!(
                    self,
                    insert_keymaps,
                    fallback = self.fallback_insert(buf, ch)
                )
            }
            ModeState::Command { cmdline } => handle_mode!(
                self,
                command_keymaps,
                fallback = {
                    match ch {
                        Input::Char(ch) => cmdline.push(ch),
                        _ => todo!(),
                    }
                }
            ),

            ModeState::Visual => handle_mode!(self, visual_keymaps),
        }
        if self.state.quit {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }

    pub fn mode(&self) -> Mode {
        (&self.state.mode).into()
    }
}

impl Vim {
    fn configure_normal_mode(&mut self) {
        let mode = Mode::Normal;
        use CursorDirection as C;
        use Input as I;
        self.add_keymap(mode, [I::Escape], |mut a| a.set_mode(ModeState::Normal));
        self.add_keymap(mode, [I::Char(ctrl_key(b'u') as char)], |a| {
            for _ in 0..12 {
                a.buf.move_cursor(C::Up)
            }
        });
        self.add_keymap(mode, [I::Char(ctrl_key(b'd') as char)], |a| {
            for _ in 0..12 {
                a.buf.move_cursor(C::Down)
            }
        });
        self.add_keymap(mode, [I::Char('h')], |a| a.buf.move_cursor(C::Left));
        self.add_keymap(mode, [I::Char('j')], |a| a.buf.move_cursor(C::Down));
        self.add_keymap(mode, [I::Char('k')], |a| a.buf.move_cursor(C::Up));
        self.add_keymap(mode, [I::Char('l')], |a| a.buf.move_cursor(C::Right));
        self.add_keymap(mode, [I::Char('i')], |mut a| a.set_mode(ModeState::Insert));
        self.add_keymap(mode, [I::Char('v')], |mut a| a.set_mode(ModeState::Visual));
        self.add_keymap(mode, [I::Char('g'), I::Char('g')], |a| {
            buf_seek_line(a.buf, 0)
        });
        self.add_keymap(mode, [I::Char('G')], |a| {
            buf_seek_line(a.buf, a.buf.num_lines())
        });
        self.add_keymap(mode, [I::Char('a')], |mut a| {
            let (line, col) = a.buf.position().destruct();
            a.set_mode(ModeState::Insert);
            a.buf.set_position(line, col + 1);
        });
        self.add_keymap(mode, [I::Char('o')], |mut a| {
            let line = a.buf.position().line();
            a.buf.set_lines(line + 1, [""]);
            a.buf.set_position(line + 1, 0);

            a.set_mode(ModeState::Insert);
        });
        self.add_keymap(mode, [I::Char('0')], |a| {
            let line = a.buf.position().line();
            a.buf.set_position(line, 0);
        });
        self.add_keymap(mode, [I::Char('$')], |a| {
            let line = a.buf.position().line();
            let last = a.buf.get_row(line).unwrap_or("").len();
            a.buf.set_position(line, last);
        });
        self.add_keymap(mode, [I::Char('y'), I::Char('y')], |a| {
            let line = a.buf.position().line();
            let line = a.buf.get_row(line).unwrap_or("").to_owned();
            a.state.registers.set_register('"', line, true);
        });
        self.add_keymap(mode, [I::Char('d'), I::Char('d')], |a| {
            let line = a.buf.position().line();
            let content = a.buf.remove_line(line);
            a.state.registers.set_register('"', content, true);
        });
        self.add_keymap(mode, [I::Char('x')], |a| {
            let pos = a.buf.position();
            a.buf
                .delete_range(pos, Location::new(pos.line(), pos.col() + 1));
        });
        self.add_keymap(mode, [I::Char('p')], |a| {
            // TODO: not quite correct
            let line = a.buf.position().line();
            let reg = a.state.registers.get_register('"');
            if reg.yanked_linewise {
                a.buf.set_lines(line + 1, reg.value.lines());
                a.buf.set_position(line + 1, 0);
                return;
            }
            // TODO: use `set_range` API instead
            for ch in reg.chars() {
                if ch == '\n' {
                    a.buf.add_newline();
                } else {
                    a.buf.insert_char(ch);
                }
            }
            if reg.yanked_linewise {
                a.buf.set_position(line + 1, 0);
            }
        });
        self.configure_simple_motion([I::Char('w')], motion::Word::new());
        self.configure_simple_motion([I::Char('W')], motion::BigWord::new());
        self.configure_simple_motion([I::Char('b')], motion::Back::new());
        self.configure_simple_motion([I::Char('B')], motion::BigBack::new());

        fn sort_location(start: Location, end: Location) -> (Location, Location) {
            if end < start {
                (end, start)
            } else {
                (start, end)
            }
        }

        self.configure_motions(&[I::Char('d')], |a, start, end| {
            let (start, end) = sort_location(start, end);
            debug!("delete {start:?} {end:?}");
            let s = join_iter(a.buf.get_range(start, end));
            a.buf.delete_range(start, end);
            a.state.registers.set_register('"', s, false);
        });
        self.configure_motions(&[I::Char('y')], |a, start, end| {
            let (start, end) = sort_location(start, end);
            let s = join_iter(a.buf.get_range(start, end));
            a.state.registers.set_register('"', s, false);
        });
        self.add_keymap(mode, [I::Char(':')], |mut a| {
            a.set_mode(ModeState::Command {
                cmdline: String::new(),
            })
        });
        self.configure_arrow_keys(mode);
    }

    fn configure_simple_motion<M>(&mut self, mapping: impl IntoIterator<Item = Input>, motion: M)
    where
        M: Motion + 'static,
    {
        let mode = Mode::Normal;
        self.add_keymap(mode, mapping, move |a| {
            if let Some(next) = motion.next(a.buf) {
                a.buf.set_position(next.line(), next.col());
            }
        });
    }

    fn configure_motion<F>(
        &mut self,
        prefix: &[Input],
        suffix: impl IntoIterator<Item = Input>,
        motion: impl Motion + 'static,
        f: F,
    ) where
        F: Fn(MapArgs, Location, Location) + 'static,
    {
        let mode = Mode::Normal;
        self.add_keymap(mode, prefix.iter().copied().chain(suffix), move |a| {
            let start = a.buf.position();
            let Some(end) = motion.next(a.buf) else {
                return;
            };
            f(a, start, end)
        });
    }

    fn configure_motions<F>(&mut self, prefix: &[Input], f: F)
    where
        F: Fn(MapArgs, Location, Location) + 'static + Clone,
    {
        use Input as I;
        self.configure_motion(prefix, [I::Char('w')], Word::new(), f.clone());
        self.configure_motion(prefix, [I::Char('W')], BigWord::new(), f.clone());
        self.configure_motion(prefix, [I::Char('b')], Back::new(), f.clone());
        self.configure_motion(prefix, [I::Char('B')], BigBack::new(), f.clone());
    }

    fn configure_insert_mode(&mut self) {
        let mode = Mode::Insert;
        use Input as I;
        self.add_keymap(mode, [I::Escape], |mut a| a.set_mode(ModeState::Normal));
        self.configure_arrow_keys(mode);
        self.add_keymap(mode, [I::Backspace], |a| a.buf.delete_char());
        self.add_keymap(mode, [I::Enter], |a| a.buf.add_newline());
    }

    fn configure_command_mode(&mut self) {
        let mode = Mode::Command;
        use Input as I;

        self.add_keymap(mode, [I::Escape], |mut a| a.set_mode(ModeState::Normal));
        self.add_keymap(mode, [I::Enter], |a| a.state.execute_cmd(a.buf));
        self.add_keymap(mode, [I::Backspace], |mut a| {
            let s = a.state.mode.expect_command();
            if s.pop().is_none() {
                a.set_mode(ModeState::Normal);
            }
        });
        {
            use CursorDirection as C;
            for dir in [C::Left, C::Right] {
                self.add_keymap(mode, [I::Arrow(dir)], move |_| debug!("move {dir:?}"));
            }
        };
    }

    fn configure_arrow_keys(&mut self, mode: Mode) {
        use CursorDirection as C;
        use Input as I;
        for dir in [C::Left, C::Right, C::Up, C::Down] {
            self.add_keymap(mode, [I::Arrow(dir)], move |a| a.buf.move_cursor(dir));
        }
    }

    fn configure_visual_mode(&mut self) {
        let mode = Mode::Visual;
        use CursorDirection as C;
        use Input as I;
        self.add_keymap(mode, [I::Escape], |mut a| a.set_mode(ModeState::Normal));
        self.configure_arrow_keys(mode);
        self.add_keymap(mode, [I::Backspace], |a| a.buf.delete_char());
        self.add_keymap(mode, [I::Enter], |a| a.buf.add_newline());
        self.add_keymap(mode, [I::Char('h')], |a| a.buf.move_cursor(C::Left));
        self.add_keymap(mode, [I::Char('j')], |a| a.buf.move_cursor(C::Down));
        self.add_keymap(mode, [I::Char('k')], |a| a.buf.move_cursor(C::Up));
        self.add_keymap(mode, [I::Char('l')], |a| a.buf.move_cursor(C::Right));
    }
}

fn buf_seek_line(buf: &mut Buffer, line: usize) {
    let cur = buf.position();
    buf.set_position(line, cur.col());
}

impl VimState {
    pub fn new() -> Self {
        Self {
            mode: ModeState::Normal,
            quit: false,
            registers: RegisterFile::new(),
        }
    }

    fn execute_cmd(&mut self, buf: &mut Buffer) {
        let mut mode = std::mem::take(&mut self.mode);
        let cmdline = mode.expect_command();
        fn write(buf: &mut Buffer) {
            let Some(path) = buf.path() else {
                return;
            };
            let path = path.to_owned();
            buf.scrub();
            let s = buf.save();
            std::fs::write(path, &s).expect("cant write");
        }
        fn quit(state: &mut VimState) {
            state.quit = true;
        }
        // TODO: implement vimL?
        match cmdline.trim() {
            "wq" | "wqa" => {
                write(buf);
                quit(self);
            }
            "w" => {
                write(buf);
            }
            "qa" | "qa!" | "q" | "q!" => quit(self),
            s if let Some(filename) = s.strip_prefix("f ") => {
                buf.set_path(filename.to_owned());
                buf.set_name(filename.to_owned());
            }
            s if let Some(command) = s.strip_prefix("!") => {
                let shell = env::var("SHELL").unwrap_or_else(|_| String::from("sh"));
                let res = Command::new(shell).arg("-c").arg(command).output();
                debug!("!{command} => {res:?}");
            }
            // :<num> => seek to line
            s if let Ok(line) = s.parse::<usize>() => {
                buf_seek_line(buf, line);
            }
            _ => debug!("TODO: notify that this command ({cmdline}) is unknown"),
        }
    }

    fn set_mode(&mut self, mode: ModeState, buf: &mut Buffer) {
        match mode {
            ModeState::Normal | ModeState::Visual | ModeState::Command { .. } => {
                buf.set_go_past_end(false)
            }
            ModeState::Insert => buf.set_go_past_end(true),
        }
        self.mode = mode;
    }
}

impl RegisterFile {
    pub fn get_register(&self, name: char) -> &RegisterEntry {
        assert_eq!(name, '"', "currently only unnamed (\") is supported");
        &self.unnamed
    }

    pub fn set_register(&mut self, name: char, s: String, linewise: bool) {
        assert_eq!(name, '"', "currently only unnamed (\") is supported");
        self.unnamed.value = s;
        self.unnamed.yanked_linewise = linewise;
    }

    fn new() -> Self {
        Self {
            unnamed: RegisterEntry::new(String::new()),
        }
    }
}

fn join_iter<'a>(it: impl Iterator<Item = &'a str>) -> String {
    let mut ret = String::new();
    let mut needs_newline = false;
    for line in it {
        if needs_newline {
            ret.push('\n');
        }
        needs_newline = true;
        ret.push_str(line);
    }
    ret
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use crate::location::Location;

    use super::*;

    #[test]
    fn handle_key() {
        let mut v = Vim::bare();
        let mut buf = Buffer::new();
        let x = Rc::new(Cell::new(0));
        let y = Rc::clone(&x);
        v.add_keymap(
            Mode::Normal,
            [Input::Char('a'), Input::Char('b')],
            move |_| x.set(1),
        );

        assert_eq!(y.get(), 0);

        let _ = v.handle_input(&mut buf, Input::Char('a'));
        assert_eq!(y.get(), 0);

        let _ = v.handle_input(&mut buf, Input::Char('b'));

        assert_eq!(y.get(), 1);
    }

    #[test]
    fn handle_key_unknown_sequence() {
        let mut v = Vim::bare();
        let mut buf = Buffer::new();
        let x = Rc::new(Cell::new(0));
        let y = Rc::clone(&x);
        let z = Rc::clone(&x);
        v.add_keymap(
            Mode::Normal,
            [Input::Char('a'), Input::Char('b')],
            move |_| y.set(1),
        );
        v.add_keymap(Mode::Normal, [Input::Char('c')], move |_| z.set(2));

        assert_eq!(x.get(), 0);

        let _ = v.handle_input(&mut buf, Input::Char('a'));
        assert_eq!(x.get(), 0);

        let _ = v.handle_input(&mut buf, Input::Char('c'));
        assert_eq!(x.get(), 0);

        let _ = v.handle_input(&mut buf, Input::Char('c'));

        assert_eq!(x.get(), 2);
    }

    #[test]
    fn mode_str() {
        assert_eq!(Mode::Normal.str(), "NORMAL");
        assert_eq!(Mode::Insert.str(), "INSERT");
        assert_eq!(Mode::Visual.str(), "VISUAL");
    }

    trait NoBreak {
        fn no_break(self);
        fn breaks(self);
    }
    impl NoBreak for ControlFlow<()> {
        fn no_break(self) {
            assert!(!self.is_break());
        }
        fn breaks(self) {
            assert!(self.is_break());
        }
    }

    fn buffer(s: &str) -> (tempfile::NamedTempFile, Buffer) {
        let file = tempfile::NamedTempFile::new().unwrap();
        let buf = Buffer::read(file.path().to_str().unwrap().to_owned(), s);
        (file, buf)
    }

    fn feedkeys(vim: &mut Vim, buf: &mut Buffer, keys: &str) -> ControlFlow<()> {
        for ch in keys.chars() {
            let ch = match ch {
                '\n' => Input::Enter,
                _ => {
                    assert!(ch.is_ascii());
                    assert!(!ch.is_ascii_control());
                    Input::Char(ch)
                }
            };
            vim.handle_input(buf, ch)?;
        }
        ControlFlow::Continue(())
    }

    #[test]
    fn insert_works() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello world");
        vim.handle_input(&mut buf, Input::Char('l')).no_break();
        assert_eq!(buf.position(), (0, 1).into());
        vim.handle_input(&mut buf, Input::Char('i')).no_break();
        assert_eq!(vim.mode(), Mode::Insert);
        vim.handle_input(&mut buf, Input::Char(' ')).no_break();
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(buf.save(), "h ello world\n");
    }

    #[test]
    fn a_works() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello world");
        feedkeys(&mut vim, &mut buf, "lllla,").no_break();
        assert_eq!(buf.save(), "hello, world\n");
        assert_eq!(vim.mode(), Mode::Insert);
        vim.handle_input(&mut buf, Input::Escape).no_break();
        assert_eq!(vim.mode(), Mode::Normal);

        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello, world");
        feedkeys(&mut vim, &mut buf, "$a!").no_break();
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(buf.save(), "hello, world!\n");
    }

    #[test]
    fn motion_works() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello\nworld\nfoo\nbar");
        feedkeys(&mut vim, &mut buf, "llj").no_break();
        assert_eq!(buf.position(), Location::new(1, 2));
        feedkeys(&mut vim, &mut buf, "k").no_break();
        assert_eq!(buf.position(), Location::new(0, 2));
        feedkeys(&mut vim, &mut buf, "h").no_break();
        assert_eq!(buf.position(), Location::new(0, 1));
        feedkeys(&mut vim, &mut buf, "lllllll").no_break();
        assert_eq!(buf.position(), Location::new(0, 4));
    }

    #[test]
    fn yank_works() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello\nworld\nfoo\nbar");
        assert_eq!(buf.position(), Location::new(0, 0));
        feedkeys(&mut vim, &mut buf, "yyjp").no_break();
        assert_eq!(buf.save(), "hello\nworld\nhello\nfoo\nbar\n");
        assert_eq!(buf.position(), Location::new(2, 0));
    }

    #[test]
    fn w_works() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello  world");
        feedkeys(&mut vim, &mut buf, "w").no_break();
        assert_eq!(buf.position(), Location::new(0, 7));
        feedkeys(&mut vim, &mut buf, "w").no_break();
        assert_eq!(buf.position(), Location::new(0, 11));
    }

    #[test]
    fn o_works() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello  world");
        feedkeys(&mut vim, &mut buf, "o").no_break();
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(buf.position(), Location::new(1, 0));
        assert_eq!(buf.save(), "hello  world\n\n");

        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("");
        feedkeys(&mut vim, &mut buf, "o").no_break();
        assert_eq!(buf.position(), Location::new(0, 0));
    }

    #[test]
    fn insert_newline_works() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("helloworld");
        feedkeys(&mut vim, &mut buf, "llllli\n").no_break();
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(buf.position(), Location::new(1, 0));
        assert_eq!(buf.save(), "hello\nworld\n");
    }

    #[test]
    fn quit_works() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("helloworld");
        feedkeys(&mut vim, &mut buf, ":q").no_break();
        feedkeys(&mut vim, &mut buf, "\n").breaks();
    }

    #[test]
    fn command_mode() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("helloworld");
        feedkeys(&mut vim, &mut buf, ":f").no_break();
        assert_eq!(vim.command_str(), Some("f"));
        vim.handle_input(&mut buf, Input::Backspace).no_break();
        assert_eq!(vim.command_str(), Some(""));
        assert_eq!(vim.mode(), Mode::Command);
        vim.handle_input(&mut buf, Input::Backspace).no_break();
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn write_works() {
        let mut vim = Vim::new();
        let (f, mut buf) = buffer("helloworld");
        feedkeys(&mut vim, &mut buf, ":w\n").no_break();
        let s = std::io::read_to_string(f).unwrap();
        assert_eq!(s, "helloworld\n");
    }

    #[test]
    fn write_quit_works() {
        let mut vim = Vim::new();
        let (f, mut buf) = buffer("helloworld");
        feedkeys(&mut vim, &mut buf, ":wq").no_break();
        feedkeys(&mut vim, &mut buf, "\n").breaks();
        let s = std::io::read_to_string(f).unwrap();
        assert_eq!(s, "helloworld\n");
    }

    #[test]
    fn page_up_down() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello\nworld\nfoo");
        let before = buf.position().line();
        vim.handle_input(&mut buf, Input::Char(ctrl_key(b'd') as char))
            .no_break();
        let after = buf.position().line();
        assert!(before < after);

        vim.handle_input(&mut buf, Input::Char(ctrl_key(b'u') as char))
            .no_break();
        let last = buf.position().line();
        assert!(after > last);
    }

    #[test]
    fn yank() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello world");
        feedkeys(&mut vim, &mut buf, "yw").no_break();
        assert_eq!(
            vim.state.registers.get_register('"'),
            &RegisterEntry::new("hello ".into())
        );
        assert_eq!(buf.position(), Location::new(0, 0));
    }

    #[test]
    fn delete() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello world");
        feedkeys(&mut vim, &mut buf, "dw").no_break();
        assert_eq!(
            vim.state.registers.get_register('"'),
            &RegisterEntry::new("hello ".into())
        );
        assert_eq!(buf.save(), "world\n");
        assert_eq!(buf.position(), Location::new(0, 0));
    }

    #[test]
    fn delete_back() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello world");
        feedkeys(&mut vim, &mut buf, "lllldb").no_break();
        assert_eq!(
            vim.state.registers.get_register('"'),
            &RegisterEntry::new("hell".into())
        );
        assert_eq!(buf.save(), "o world\n");
        assert_eq!(buf.position(), Location::new(0, 0));
    }

    #[test]
    fn end_of_line() {
        let mut vim = Vim::new();
        let (_f, mut buf) = buffer("hello world");
        feedkeys(&mut vim, &mut buf, "$").no_break();
        assert_eq!(buf.position(), Location::new(0, 10));
        feedkeys(&mut vim, &mut buf, "0").no_break();
        assert_eq!(buf.position(), Location::new(0, 0));
    }
}
