//! Software write-combining partitioner.
//!
//! Records bound for P partitions are first staged in a small per-partition buffer of K records
//! (all stages together stay in L2), then copied to the partition's output in runs of K. Every record
//! is written once to its final place, the writes to memory are long contiguous runs, and the output of
//! one thread is one contiguous vector per partition (few, long slices for the next phase to read).

pub struct Swwc<T: Copy + Default, const K: usize> {
    stage: Vec<[T; K]>,
    fill: Vec<u32>,
    pub out: Vec<Vec<T>>,
}

impl<T: Copy + Default, const K: usize> Swwc<T, K> {
    /// `reserve` records of capacity per partition (an estimate; outputs grow past it if needed).
    pub fn new(parts: usize, reserve: usize) -> Self {
        Swwc {
            stage: vec![[T::default(); K]; parts],
            fill: vec![0; parts],
            out: (0..parts).map(|_| Vec::with_capacity(reserve)).collect(),
        }
    }

    #[inline(always)]
    pub fn push(&mut self, part: usize, rec: T) {
        debug_assert!(part < self.fill.len());
        // SAFETY: `part` is a partition index below `fill.len() == stage.len() == out.len()`, and the
        // fill count is always below K.
        unsafe {
            let f = self.fill.get_unchecked_mut(part);
            let stage = self.stage.get_unchecked_mut(part);
            *stage.get_unchecked_mut(*f as usize) = rec;
            *f += 1;
            if *f as usize == K {
                *f = 0;
                self.out.get_unchecked_mut(part).extend_from_slice(stage);
            }
        }
    }

    /// Moves the partly filled stages to the outputs.
    pub fn finish(&mut self) {
        for (p, out) in self.out.iter_mut().enumerate() {
            let f = self.fill[p] as usize;
            out.extend_from_slice(&self.stage[p][..f]);
            self.fill[p] = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_record_lands_in_its_partition_in_order() {
        let mut w: Swwc<u64, 8> = Swwc::new(5, 2);
        let mut want: Vec<Vec<u64>> = vec![Vec::new(); 5];
        for i in 0..1000u64 {
            let p = ((i * 2_654_435_761) % 5) as usize;
            w.push(p, i);
            want[p].push(i);
        }
        w.finish();
        assert_eq!(w.out, want);
    }
}
