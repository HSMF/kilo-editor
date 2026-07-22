#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
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

impl PartialOrd for Location {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Location {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.line.cmp(&other.line).then(self.col.cmp(&other.col))
    }
}

impl std::fmt::Debug for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { line, col } = self;
        write!(f, "Location({line}:{col})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cmp() {
        assert!(Location::new(0, 0) < Location::new(0, 1));
        assert!(Location::new(0, 10) < Location::new(1, 0));
    }
}
