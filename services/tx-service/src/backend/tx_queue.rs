use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(bound = "K: Hash + Eq + Serialize + DeserializeOwned")]
pub struct TxQueue<K> {
    index: HashMap<K, usize>,
    queue: VecDeque<K>,
}

impl<K> TxQueue<K>
where
    K: Hash + PartialEq + Eq + Clone,
{
    pub fn new() -> Self {
        Self {
            index: HashMap::new(),
            queue: VecDeque::new(),
        }
    }

    pub fn insert(&mut self, key: K) {
        self.queue.push_back(key.clone());
        self.index.insert(key, self.queue.len() - 1);
    }

    pub fn remove(&mut self, key: &K) -> bool {
        if let Some(&index) = self.index.get(key) {
            self.queue.remove(index);
            self.index.remove(key);
            return true;
        }
        false
    }

    pub fn swap(&mut self, a: &K, b: &K) {
        if let (Some(&a_index), Some(&b_index)) = (self.index.get(a), self.index.get(b)) {
            self.queue.swap(a_index, b_index);
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.index.contains_key(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = &K> {
        self.queue.iter()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }
}

impl<K> IntoIterator for TxQueue<K>
where
    K: Hash + PartialEq + Eq + Clone,
{
    type Item = K;
    type IntoIter = std::collections::vec_deque::IntoIter<K>;

    fn into_iter(self) -> Self::IntoIter {
        self.queue.into_iter()
    }
}
