//! 時間変化グラフに表示する倍音の ON／OFF。
//!
//! 倍音一覧のトグル、スペクトログラムでの範囲選択、凡例はすべてこの状態を共有する。
//! 解析範囲外の倍音を ON にしないのは呼び出し側の責任（`harmonics_between` は範囲内だけを返す）。

use std::collections::BTreeSet;

/// 初期状態で ON にする倍音の数（第1〜第8倍音）
pub const DEFAULT_ON: usize = 8;

/// 範囲選択した倍音への操作
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeOp {
    /// 選んだ倍音だけを ON にし、ほかはすべて OFF（置き換え）
    Only,
    /// 選んだ倍音を ON にし、ほかはそのまま
    Add,
    /// 選んだ倍音を OFF にし、ほかはそのまま
    Off,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown(BTreeSet<usize>);

impl Default for Shown {
    fn default() -> Self {
        Shown((1..=DEFAULT_ON).collect())
    }
}

impl Shown {
    pub fn from_iter(ks: impl IntoIterator<Item = usize>) -> Self {
        Shown(ks.into_iter().filter(|&k| k >= 1).collect())
    }

    pub fn contains(&self, k: usize) -> bool {
        self.0.contains(&k)
    }

    pub fn set(&mut self, k: usize, on: bool) {
        if on {
            self.0.insert(k);
        } else {
            self.0.remove(&k);
        }
    }

    pub fn all_off(&mut self) {
        self.0.clear();
    }

    /// 初期状態（第1〜第8倍音が ON、第9倍音以降は OFF）に戻す
    pub fn reset(&mut self) {
        *self = Shown::default();
    }

    pub fn apply(&mut self, ks: &[usize], op: RangeOp) {
        match op {
            RangeOp::Only => {
                self.0.clear();
                self.0.extend(ks.iter().copied().filter(|&k| k >= 1));
            }
            RangeOp::Add => self.0.extend(ks.iter().copied().filter(|&k| k >= 1)),
            RangeOp::Off => ks.iter().for_each(|k| {
                self.0.remove(k);
            }),
        }
    }

    /// ON の倍音のうち、第1〜第`num_in_range`倍音（解析範囲内）のもの
    pub fn in_range(&self, num_in_range: usize) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().copied().take_while(move |&k| k <= num_in_range)
    }
}

/// 周波数 `a`〜`b`（向きは問わない、両端を含む）に入る、解析範囲内の倍音
pub fn harmonics_between(f0: f64, a: f64, b: f64, num_in_range: usize) -> Vec<usize> {
    let (lo, hi) = (a.min(b), a.max(b));
    if f0 <= 0.0 {
        return Vec::new();
    }
    (1..=num_in_range).filter(|&k| (lo..=hi).contains(&(k as f64 * f0))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(s: &Shown, n: usize) -> Vec<usize> {
        s.in_range(n).collect()
    }

    #[test]
    fn default_is_first_eight() {
        assert_eq!(on(&Shown::default(), 100), (1..=8).collect::<Vec<_>>());
    }

    #[test]
    fn range_select_matches_task_example() {
        // 220 Hz の第7〜第9倍音（1540, 1760, 1980 Hz）を、どちら向きのドラッグでも同じに選ぶ
        let ks = harmonics_between(220.0, 1450.0, 2000.0, 100);
        assert_eq!(ks, vec![7, 8, 9]);
        assert_eq!(harmonics_between(220.0, 2000.0, 1450.0, 100), ks);

        let mut s = Shown::default();
        s.apply(&ks, RangeOp::Only);
        assert_eq!(on(&s, 100), vec![7, 8, 9]);

        let mut s = Shown::default();
        s.apply(&ks, RangeOp::Add);
        assert_eq!(on(&s, 100), (1..=9).collect::<Vec<_>>());

        let mut s = Shown::default();
        s.apply(&ks, RangeOp::Off);
        assert_eq!(on(&s, 100), (1..=6).collect::<Vec<_>>());
    }

    #[test]
    fn out_of_range_harmonics_are_never_picked() {
        // 範囲内が第5倍音までなら、第7〜第9倍音の帯を選んでも何も入らない
        assert!(harmonics_between(220.0, 1450.0, 2000.0, 5).is_empty());
        assert_eq!(on(&Shown::default(), 5), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn toggles_and_bulk_ops_share_state() {
        let mut s = Shown::default();
        s.set(3, false);
        s.set(12, true);
        assert_eq!(on(&s, 100), vec![1, 2, 4, 5, 6, 7, 8, 12]);
        s.all_off();
        assert_eq!(on(&s, 100), Vec::<usize>::new());
        s.set(3, true);
        assert_eq!(on(&s, 100), vec![3]);
        s.reset();
        assert_eq!(s, Shown::default());
    }
}
