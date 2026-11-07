//! Bounded preview sampled from the actual, top-to-bottom output tiles.
//! Global pixel-centre sampling makes the result independent of tile layout.

pub type ExportFrame = (Vec<u8>, u32, u32);

pub struct Thumbnail {
    source: [u32; 2],
    size: [u32; 2],
    columns: Vec<u32>,
    rows: Vec<u32>,
    pixels: Vec<u8>,
    filled: Vec<bool>,
}

impl Thumbnail {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("导出缩略图尺寸无效".into());
        }
        let scale = (960.0 / f64::from(width.max(height))).min(1.0);
        let size = [
            (f64::from(width) * scale).round().max(1.0) as u32,
            (f64::from(height) * scale).round().max(1.0) as u32,
        ];
        let positions = |source: u32, target: u32| {
            (0..target)
                .map(|index| {
                    ((2 * u64::from(index) + 1) * u64::from(source) / (2 * u64::from(target)))
                        as u32
                })
                .collect()
        };
        let count = size[0] as usize * size[1] as usize;
        Ok(Self {
            source: [width, height],
            size,
            columns: positions(width, size[0]),
            rows: positions(height, size[1]),
            pixels: vec![0; count * 4],
            filled: vec![false; count],
        })
    }

    pub fn add_tile(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<(), String> {
        let end_x = x.checked_add(width).ok_or("导出缩略图分块范围无效")?;
        let end_y = y.checked_add(height).ok_or("导出缩略图分块范围无效")?;
        let expected = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|count| count.checked_mul(4))
            .ok_or("导出缩略图分块尺寸无效")?;
        if width == 0
            || height == 0
            || end_x > self.source[0]
            || end_y > self.source[1]
            || expected != pixels.len() as u64
        {
            return Err("导出缩略图分块尺寸无效".into());
        }
        let first_x = self.columns.partition_point(|&value| value < x);
        let last_x = self.columns.partition_point(|&value| value < end_x);
        let first_y = self.rows.partition_point(|&value| value < y);
        let last_y = self.rows.partition_point(|&value| value < end_y);
        for row in first_y..last_y {
            for column in first_x..last_x {
                let source_index = ((self.rows[row] - y) as usize * width as usize
                    + (self.columns[column] - x) as usize)
                    * 4;
                let target_index = row * self.size[0] as usize + column;
                self.pixels[target_index * 4..target_index * 4 + 4]
                    .copy_from_slice(&pixels[source_index..source_index + 4]);
                self.filled[target_index] = true;
            }
        }
        Ok(())
    }

    pub fn finish(self) -> Result<ExportFrame, String> {
        if self.filled.iter().any(|&value| !value) {
            return Err("导出缩略图尚未完整".into());
        }
        Ok((self.pixels, self.size[0], self.size[1]))
    }
}


