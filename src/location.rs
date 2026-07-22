#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Location {
    line: usize,
    col: usize,
}

impl From<(usize, usize)> for Location {
    fn from((line, col): (usize, usize)) -> Self {
        Self::new(line, col)
    }
}

impl Location {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }

    pub fn line(&self) -> usize {
        self.line
    }

    pub fn col(&self) -> usize {
        self.col
    }

    pub fn destruct(&self) -> (usize, usize) {
        (self.line, self.col)
    }
}
