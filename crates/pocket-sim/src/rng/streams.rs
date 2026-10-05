//! The generators of one tick by derived seed: an open-addressing table with linear probing,
//! cleared in constant time at the end of each tick by a generation stamp. It offers lookups only,
//! no iteration, so its layout never reaches a result (simulation.md 8.4 allows a hash table for
//! lookups behind such a type); the derived seeds are SplitMix outputs, so their low bits index it
//! directly. A `BTreeMap` cost about 200 ns a stream at 10,000 entity streams a tick.

use super::Pcg32;

#[derive(Clone, Copy, Debug)]
struct Slot {
    /// The table's generation when written; any other value is an empty slot.
    generation: u32,
    key: u64,
    generator: Pcg32,
}

const EMPTY: Slot = Slot {
    generation: 0,
    key: 0,
    generator: Pcg32 { state: 0, inc: 1 },
};

#[derive(Debug)]
pub(crate) struct StreamMap {
    slots: Vec<Slot>,
    len: usize,
    generation: u32,
}

impl Default for StreamMap {
    fn default() -> Self {
        StreamMap {
            slots: Vec::new(),
            len: 0,
            generation: 1,
        }
    }
}

impl StreamMap {
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Empties the table in constant time.
    pub(crate) fn clear(&mut self) {
        self.len = 0;
        if self.generation == u32::MAX {
            self.slots.fill(EMPTY);
            self.generation = 1;
        } else {
            self.generation += 1;
        }
    }

    fn mask(&self) -> usize {
        self.slots.len() - 1
    }

    fn home(&self, key: u64) -> usize {
        // Truncating to the table's index width is the point.
        #[allow(clippy::cast_possible_truncation)]
        let k = key as usize;
        k & self.mask()
    }

    fn live(&self, i: usize) -> bool {
        self.slots[i].generation == self.generation
    }

    /// The slot holding `key`, or the empty slot where it would go.
    fn find(&self, key: u64) -> Result<usize, usize> {
        let mut i = self.home(key);
        loop {
            if !self.live(i) {
                return Err(i);
            }
            if self.slots[i].key == key {
                return Ok(i);
            }
            i = (i + 1) & self.mask();
        }
    }

    fn grow(&mut self) {
        let cap = (self.slots.len() * 2).max(64);
        let old = std::mem::replace(&mut self.slots, vec![EMPTY; cap]);
        let generation = self.generation;
        for s in old.into_iter().filter(|s| s.generation == generation) {
            if let Err(i) = self.find(s.key) {
                self.slots[i] = s;
            }
        }
    }

    pub(crate) fn get(&self, key: u64) -> Option<Pcg32> {
        if self.len == 0 {
            return None;
        }
        self.find(key).ok().map(|i| self.slots[i].generator)
    }

    /// The generator of `key`, made by `make` if absent.
    pub(crate) fn get_or_insert(&mut self, key: u64, make: impl FnOnce() -> Pcg32) -> &mut Pcg32 {
        if (self.len + 1) * 2 > self.slots.len() {
            self.grow();
        }
        let i = match self.find(key) {
            Ok(i) => i,
            Err(i) => {
                self.slots[i] = Slot {
                    generation: self.generation,
                    key,
                    generator: make(),
                };
                self.len += 1;
                i
            }
        };
        &mut self.slots[i].generator
    }

    /// Sets the generator of `key`.
    pub(crate) fn insert(&mut self, key: u64, generator: Pcg32) {
        *self.get_or_insert(key, || generator) = generator;
    }

    /// Removes `key`, shifting later entries of its probe run back so lookups still find them.
    pub(crate) fn remove(&mut self, key: u64) {
        if self.len == 0 {
            return;
        }
        let Ok(mut hole) = self.find(key) else {
            return;
        };
        let mask = self.mask();
        let mut j = hole;
        loop {
            j = (j + 1) & mask;
            if !self.live(j) {
                break;
            }
            let home = self.home(self.slots[j].key);
            // Move j into the hole unless its home lies cyclically in (hole, j].
            let stays = if hole <= j {
                hole < home && home <= j
            } else {
                hole < home || home <= j
            };
            if !stays {
                self.slots[hole] = self.slots[j];
                hole = j;
            }
        }
        self.slots[hole] = EMPTY;
        self.len -= 1;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn behaves_like_a_map() {
        let mut m = StreamMap::default();
        let mut reference: BTreeMap<u64, Pcg32> = BTreeMap::new();
        let mut keys = Pcg32::new(5, 6);
        for round in 0..4u64 {
            for i in 0..3000u64 {
                // Clustered keys (many sharing low bits) stress the probing.
                let k = if i % 3 == 0 { i << 20 } else { keys.next_u64() };
                let g = Pcg32::new(k, round);
                m.insert(k, g);
                reference.insert(k, g);
                if i % 7 == 0 {
                    let victim = if i % 2 == 0 { k } else { (i / 2) << 20 };
                    m.remove(victim);
                    reference.remove(&victim);
                }
            }
            assert_eq!(m.len(), reference.len());
            for (k, g) in &reference {
                assert_eq!(m.get(*k), Some(*g));
            }
            assert_eq!(m.get(u64::MAX - 1), None);
            m.clear();
            reference.clear();
            assert_eq!(m.len(), 0);
            assert_eq!(m.get(3 << 20), None);
        }
    }
}
