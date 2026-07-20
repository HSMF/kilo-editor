use tinyvec::{TinyVec, tiny_vec};

/// only stores unique strings, without any common prefixes
pub struct Trie<K: Default, V, const N: usize = 4> {
    root: TrieNode<K, V, N>,
}

#[expect(unused)]
pub struct Subtrie<'a, K: Default, V, const N: usize> {
    node: &'a TrieNode<K, V, N>,
}

#[derive(Debug, Default)]
enum TrieNodeSlot<K: Default, V, const N: usize> {
    #[default]
    /// only exists so that we can put this in a tinyvec
    Nothing,
    Value(V),
    Next(Box<TrieNode<K, V, N>>),
}

#[derive(Debug)]
struct TrieNode<K: Default, V, const N: usize> {
    table: TinyVec<[(K, TrieNodeSlot<K, V, N>); N]>,
}

impl<K: Default + Eq, V, const N: usize> TrieNode<K, V, N> {
    fn index_of_key(&self, key: &K) -> Option<usize> {
        self.table
            .iter()
            .enumerate()
            .find(|(_, (k, _))| key == k)
            .map(|(i, _)| i)
    }

    pub fn get<'a>(
        &'a self,
        key: impl IntoIterator<Item = &'a K> + 'a,
    ) -> Option<GetResult<'a, K, V, N>> {
        let mut cur = Some(self);
        let mut value = None;
        for k in key {
            let current = cur?;
            let idx = current.index_of_key(k)?;
            match &current.table[idx].1 {
                TrieNodeSlot::Nothing => unreachable!("never actively constructed"),
                TrieNodeSlot::Value(v) => {
                    cur = None;
                    value = Some(v);
                }
                TrieNodeSlot::Next(trie_node) => cur = Some(trie_node),
            };
        }

        if let Some(cur) = cur {
            Some(GetResult::Subtrie(Subtrie { node: cur }))
        } else if let Some(v) = value {
            Some(GetResult::Value(v))
        } else {
            None
        }
    }
}

impl<K: Default, V> Trie<K, V> {
    pub fn new() -> Self {
        Self {
            root: TrieNode { table: tiny_vec!() },
        }
    }
}

impl<K: Default, V> Default for Trie<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

pub enum GetResult<'a, K: Default, V, const N: usize> {
    Subtrie(Subtrie<'a, K, V, N>),
    Value(&'a V),
}

impl<K: Default + Eq, V, const N: usize> Trie<K, V, N> {
    /**
     * `key` must contain at least one item
     */
    pub fn insert(&mut self, key: impl IntoIterator<Item = K>, v: V) -> Option<V> {
        let mut cur = &mut self.root;
        let mut key = key.into_iter().peekable();
        assert!(key.peek().is_some());
        let mut last = None;
        while let Some(k) = key.next() {
            if key.peek().is_some() {
                let (_, next) = if let Some(idx) = cur.index_of_key(&k) {
                    &mut cur.table[idx]
                } else {
                    cur.table.push((
                        k,
                        TrieNodeSlot::Next(Box::new(TrieNode { table: tiny_vec!() })),
                    ));
                    cur.table.last_mut().unwrap()
                };
                match next {
                    TrieNodeSlot::Next(trie_node) => cur = trie_node,
                    _ => unreachable!(),
                }
            } else {
                last = Some(k);
            }
        }
        let last = last.expect("at least one key");
        if let Some(idx) = cur.index_of_key(&last) {
            let mut tmp = TrieNodeSlot::Value(v);
            std::mem::swap(&mut tmp, &mut cur.table[idx].1);
            match tmp {
                TrieNodeSlot::Value(x) => Some(x),
                _ => todo!(),
            }
        } else {
            cur.table.push((last, TrieNodeSlot::Value(v)));
            None
        }
    }

    pub fn get<'a>(
        &'a self,
        k: impl IntoIterator<Item = &'a K> + 'a,
    ) -> Option<GetResult<'a, K, V, N>> {
        self.root.get(k)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn it_works() {
        let mut x = Trie::new();
        x.insert(['b'], 2);
        x.insert(['a', 'b'], 1);
        let sub = x.get([&'a']);
        assert!(matches!(sub, Some(GetResult::Subtrie(..))));
        let res = x.get([&'a', &'b']);
        assert!(matches!(res, Some(GetResult::Value(1))));
        let res = x.get([&'b', &'a']);
        assert!(res.is_none());
        let res = x.get([&'c']);
        assert!(res.is_none());
        let res = x.get([&'b']);
        assert!(matches!(res, Some(GetResult::Value(2))));
    }

    #[test]
    fn similar_mappings() {
        let mut x = Trie::default();
        x.insert(['y', 'i', 'w'], 1);
        x.insert(['y', 'i', 'W'], 2);
        let res = x.get(['y', 'i', 'w'].iter());
        assert!(matches!(res, Some(GetResult::Value(1))));

        let res = x.get(['y', 'i', 'W'].iter());
        assert!(matches!(res, Some(GetResult::Value(2))));
    }

    #[test]
    fn replace() {
        let mut x = Trie::default();
        x.insert("foo".chars(), 1);
        assert_eq!(x.insert("foo".chars(), 2), Some(1));
    }
}
