//! Load BCPack constant tables from `bcpack-tables.bin` (same layout as VIctory's format file).

use std::path::Path;

use super::tables::{self, Bc6Ep, Bc6Ix, Bc7Ep, Bc7Ix, Plain, BUCKETS, DICTIONARY, MODELS};
use super::victory::Tables;

/// Byte length of a valid table file (see CGBJ `victory-formats::bcpack::TABLES_SIZE`).
pub const TABLES_SIZE: usize = MODELS * BUCKETS * 2
    + 256
    + 64
    + 256
    + 64
    + 64
    + 512 * 2
    + DICTIONARY * (6 + 16 + 8 + 32 + 2 + 16 + 4 + 32 + 25 + 33 + 50 + 66);

struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        if self.rest.len() < n {
            return Err("table file truncated");
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, &'static str> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, &'static str> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes(b.try_into().unwrap()))
    }
}

fn parse(bytes: &[u8]) -> Result<Tables, &'static str> {
    if bytes.len() != TABLES_SIZE {
        return Err("table file has the wrong size");
    }
    let mut r = Reader { rest: bytes };
    let mut freq = Box::new([[0u32; BUCKETS]; MODELS]);
    let mut start = [[0u32; BUCKETS]; MODELS];
    let mut recip = [[0u64; BUCKETS]; MODELS];
    for m in 0..MODELS {
        for f in freq[m].iter_mut() {
            *f = u32::from(r.u16()?);
        }
        let used = m / 16 + 2;
        let mut at = 0u32;
        for b in 0..used {
            if freq[m][b] == 0 {
                return Err("a model has an empty bucket");
            }
            start[m][b] = at;
            recip[m][b] = (1u64 << 32).div_ceil(u64::from(freq[m][b]));
            at += freq[m][b] << b.saturating_sub(1);
        }
        if at != 1 << 16 {
            return Err("a model does not tile 65536");
        }
        start[m][used..].fill(1 << 16);
    }
    let rank1 = Box::new(r.take(256)?.try_into().map_err(|_| "rank1")?);
    let rank3a = Box::new(r.take(64)?.try_into().map_err(|_| "rank3a")?);
    let rank3c = Box::new(r.take(256)?.try_into().map_err(|_| "rank3c")?);
    let rank4 = Box::new(r.take(64)?.try_into().map_err(|_| "rank4")?);
    let rank5 = Box::new(r.take(64)?.try_into().map_err(|_| "rank5")?);
    let mut ctrl7 = Box::new([0u16; 512]);
    for c in ctrl7.iter_mut() {
        *c = r.u16()?;
    }

    let mut plain = |ep: usize, ix: usize| -> Result<Box<[Plain; DICTIONARY]>, &'static str> {
        let mut out = Box::new([Plain::default(); DICTIONARY]);
        for p in out.iter_mut() {
            for v in p.ep.iter_mut().take(ep) {
                *v = r.u8()?;
            }
            for v in p.ix.iter_mut().take(ix) {
                *v = r.u8()?;
            }
        }
        Ok(out)
    };

    let bc1 = plain(6, 16)?;
    let bc3 = plain(8, 32)?;
    let bc4 = plain(2, 16)?;
    let bc5 = plain(4, 32)?;

    let mut bc6 = Box::new([(Bc6Ep::default(), Bc6Ix::default()); DICTIONARY]);
    for slot in bc6.iter_mut() {
        let _mode = r.u8()?;
        for v in slot.0.e.as_flattened_mut() {
            *v = r.u16()?;
        }
        slot.1.mode = r.u8()?;
        for v in slot.1.ix.iter_mut() {
            *v = r.u16()? as i16;
        }
    }

    let mut bc7 = Box::new([(Bc7Ep::default(), Bc7Ix::default()); DICTIONARY]);
    for slot in bc7.iter_mut() {
        slot.0.ctrl = r.u16()?;
        for v in slot.0.rows.as_flattened_mut() {
            *v = r.u16()?;
        }
        slot.1.bits = [r.u8()?, r.u8()?];
        let _ = r.u16()?;
        for v in slot.1.ix.as_flattened_mut().iter_mut().take(31) {
            *v = r.u16()? as i16;
        }
    }
    if !r.rest.is_empty() {
        return Err("table file has trailing bytes");
    }

    Ok(Tables {
        freq: Box::leak(freq),
        start,
        recip,
        rank1: Box::leak(rank1),
        rank3a: Box::leak(rank3a),
        rank3c: Box::leak(rank3c),
        rank4: Box::leak(rank4),
        rank5: Box::leak(rank5),
        ctrl7: Box::leak(ctrl7),
        bc1: Box::leak(bc1),
        bc3: Box::leak(bc3),
        bc4: Box::leak(bc4),
        bc5: Box::leak(bc5),
        bc6: Box::leak(bc6),
        bc7: Box::leak(bc7),
    })
}

/// True when the file's model frequencies match the compiled-in tables (retail `bcpack-tables.bin`).
fn matches_builtin(bytes: &[u8]) -> bool {
    if bytes.len() != TABLES_SIZE {
        return false;
    }
    let head = MODELS * BUCKETS * 2;
    bytes[..head]
        .chunks_exact(2)
        .zip(tables::FREQ.iter().flatten())
        .all(|(b, &f)| u16::from_le_bytes(b.try_into().unwrap()) == f as u16)
}

/// Parse a table file and leak it for the process lifetime.
pub fn tables_from_file(path: &Path) -> Result<&'static Tables, &'static str> {
    let bytes = std::fs::read(path).map_err(|_| "could not read table file")?;
    if matches_builtin(&bytes) {
        return Ok(super::victory::Tables::builtin());
    }
    let parsed = parse(&bytes)?;
    static SLOT: std::sync::OnceLock<&'static Tables> = std::sync::OnceLock::new();
    Ok(SLOT.get_or_init(|| Box::leak(Box::new(parsed))))
}