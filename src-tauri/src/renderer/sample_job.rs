//! A sample is publishable only after all its disjoint full-width strips finish.
#[derive(Default)]
pub(super) struct SampleJob {
    pub row: u32,
    pending_end: Option<u32>,
}

/// One fence can cover several original disjoint draws. The draw dimensions
/// stay bounded; batching only removes CPU/GPU rendezvous between them.
pub(super) struct SampleBatch {
    row: u32,
    pub end: u32,
    rows: u32,
}
impl Iterator for SampleBatch {
    type Item = (u32, u32);
    fn next(&mut self) -> Option<Self::Item> {
        if self.row >= self.end { return None; }
        let row = self.row;
        let rows = self.rows.min(self.end - row);
        self.row += rows;
        Some((row, rows))
    }
}

impl SampleJob {
    pub fn next(&self, width: u32, height: u32, tiled: bool) -> Option<(u32, u32)> {
        if width == 0 || height == 0 || self.pending_end.is_some() || self.row >= height {
            return None;
        }
        let rows = if tiled {
            (32768 / width).clamp(1, 32)
        } else {
            height
        };
        Some((self.row, rows.min(height - self.row)))
    }
    pub fn next_batch(&self, width: u32, height: u32, tiled: bool, export: bool) -> Option<SampleBatch> {
        let (row, rows) = self.next(width, height, tiled)?;
        let draws = if tiled && export { 4 } else { 1 };
        Some(SampleBatch { row, end: row.saturating_add(rows * draws).min(height), rows })
    }
    pub fn submit(&mut self, end: u32) {
        assert!(self.pending_end.is_none() && end > self.row);
        self.pending_end = Some(end);
    }
    pub fn complete(&mut self, height: u32) -> bool {
        self.row = self.pending_end.take().expect("a submitted strip");
        self.row == height
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}


