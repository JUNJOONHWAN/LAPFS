//! Checkpoint-local allocator state. No old CIB or bitmap is overwritten.
use super::*;

fn invalid(s: &str) -> TxnError {
    TxnError::SpacemanParse(format!("allocator CoW: {s}"))
}
fn put16(raw: &mut [u8], off: usize, n: u16) -> Result<(), TxnError> {
    raw.get_mut(off..off + 2)
        .ok_or_else(|| invalid("u16 bounds"))?
        .copy_from_slice(&n.to_le_bytes());
    Ok(())
}
fn ip_clear<D: WritableBlockDevice>(
    dev: &mut D,
    sm: &SpacemanView,
    cache: &mut HashMap<u64, Vec<u8>>,
    p: u64,
    bsz: usize,
) -> Result<(), TxnError> {
    let base = u64_from_le(&sm.raw, 176);
    let count = u64_from_le(&sm.raw, 152);
    let rel = p
        .checked_sub(base)
        .filter(|n| *n < count)
        .ok_or_else(|| invalid("free outside internal pool"))? as usize;
    let index = rel / (bsz * 8);
    let bit = rel % (bsz * 8);
    let table = u32_from_le(&sm.raw, 328) as usize;
    let slot = u16_from_le(&sm.raw, table + index * 2) as u64;
    if slot >= u32_from_le(&sm.raw, 164) as u64 {
        return Err(invalid("bitmap slot bounds"));
    }
    let addr = u64_from_le(&sm.raw, 168) + slot;
    if !cache.contains_key(&addr) {
        let mut b = vec![0; bsz];
        dev.read_at(addr * bsz as u64, &mut b)?;
        cache.insert(addr, b);
    }
    let b = cache
        .get_mut(&addr)
        .ok_or_else(|| invalid("missing pool bitmap"))?;
    if b[bit / 8] & (1 << (bit % 8)) == 0 {
        return Err(invalid("pool double free"));
    }
    b[bit / 8] &= !(1 << (bit % 8));
    Ok(())
}

fn queue_nodes(
    oid: u64,
    eph: &[EphemeralObj],
    bsz: usize,
    seen: &mut HashSet<u64>,
    blocks: &mut Vec<u64>,
    limit: u64,
    xid: u64,
) -> Result<(), TxnError> {
    if seen.len() >= eph.len() || !seen.insert(oid) {
        return Err(invalid("free queue cycle"));
    }
    let node = &eph
        .iter()
        .find(|e| e.oid == oid)
        .ok_or_else(|| invalid("missing free queue node"))?
        .raw;
    if node.len() != bsz {
        return Err(invalid("free queue node size"));
    }
    if u16_from_le(node, 34) == 0 {
        let entries = crate::sm_fq::parse_sm_fq_entries(node, bsz);
        if entries.len() != u32_from_le(node, 36) as usize {
            return Err(invalid("malformed free queue leaf"));
        }
        for (freed_xid, p, count) in entries {
            if freed_xid >= xid || count == 0 || count > limit.saturating_sub(blocks.len() as u64) {
                return Err(invalid("free queue range or xid"));
            }
            for i in 0..count {
                blocks.push(
                    p.checked_add(i)
                        .ok_or_else(|| invalid("queue range overflow"))?,
                );
            }
        }
    } else {
        let entries = crate::sm_fq::parse_internal_entries(node, bsz);
        if entries.len() != u32_from_le(node, 36) as usize {
            return Err(invalid("malformed free queue index"));
        }
        for (_, _, child) in entries {
            queue_nodes(child, eph, bsz, seen, blocks, limit, xid)?;
        }
    }
    Ok(())
}

pub(super) fn persist<D: WritableBlockDevice>(
    dev: &mut D,
    sm: &mut SpacemanView,
    eph: &mut Vec<EphemeralObj>,
    bitmaps: &mut HashMap<u64, Vec<u8>>,
    cibs: &mut HashMap<u64, Vec<u8>>,
    xid: u64,
) -> Result<(), TxnError> {
    let bsz = sm.block_size as usize;
    if sm.raw.len() < 336 || bsz != 4096 {
        return Err(invalid("unsupported spaceman size"));
    }
    let maps = u32_from_le(&sm.raw, 160) as usize;
    let slots = u32_from_le(&sm.raw, 164) as usize;
    let offsets = u32_from_le(&sm.raw, 328) as usize;
    let next = u32_from_le(&sm.raw, 332) as usize;
    let xids = u32_from_le(&sm.raw, 324) as usize;
    if maps == 0 || slots <= maps || slots > 65534 {
        return Err(invalid("pool bitmap geometry"));
    }
    for (at, n, width) in [(offsets, maps, 2usize), (next, slots, 2), (xids, maps, 8)] {
        if at < 336
            || n.checked_mul(width)
                .and_then(|n| at.checked_add(n))
                .is_none_or(|end| end > sm.raw.len())
        {
            return Err(invalid("pool bitmap table bounds"));
        }
    }
    let pool_count = u64_from_le(&sm.raw, 152);
    if pool_count == 0 || pool_count.div_ceil((bsz * 8) as u64) != maps as u64 {
        return Err(invalid("pool bitmap coverage"));
    }
    let mut occupied = HashSet::new();
    for i in 0..maps {
        let slot = u16_from_le(&sm.raw, offsets + i * 2) as usize;
        if slot >= slots || !occupied.insert(slot) {
            return Err(invalid("active pool bitmap alias"));
        }
    }
    let mut cursor = u16_from_le(&sm.raw, 320) as usize;
    let tail = u16_from_le(&sm.raw, 322) as usize;
    loop {
        if cursor >= slots || !occupied.insert(cursor) {
            return Err(invalid("pool bitmap freelist cycle or alias"));
        }
        let following = u16_from_le(&sm.raw, next + cursor * 2) as usize;
        if following == 65535 {
            if cursor != tail {
                return Err(invalid("pool bitmap tail mismatch"));
            }
            break;
        }
        cursor = following;
    }
    if occupied.len() != slots {
        return Err(invalid("unaccounted pool bitmap slot"));
    }
    let table = u32_from_le(&sm.raw, 80) as usize;
    let count = u32_from_le(&sm.raw, 64) as usize;
    if table
        .checked_add(count * 8)
        .is_none_or(|n| n > sm.raw.len())
    {
        return Err(invalid("CIB table bounds"));
    }
    // Queued pool blocks are no longer referenced by the active checkpoint's
    // allocator. Reuse is confined to a new pool bitmap, preserving the old one.
    let root = u64_from_le(&sm.raw, 208);
    if root != 0 {
        let mut seen = HashSet::new();
        let mut blocks = Vec::new();
        queue_nodes(
            root,
            eph,
            bsz,
            &mut seen,
            &mut blocks,
            u64_from_le(&sm.raw, 152),
            xid,
        )?;
        if blocks.len() as u64 != u64_from_le(&sm.raw, 200) {
            return Err(invalid("pool queue count mismatch"));
        }
        // Never trust a queue entry that aliases a currently referenced CIB/bitmap.
        let mut live = HashSet::new();
        for i in 0..count {
            let p = u64_from_le(&sm.raw, table + i * 8);
            live.insert(p);
            let mut buf = vec![0; bsz];
            dev.read_at(p * bsz as u64, &mut buf)?;
            let n = u32_from_le(&buf, 36) as usize;
            if 40 + n * 32 > bsz {
                return Err(invalid("CIB chunk count bounds"));
            }
            for j in 0..n {
                let b = u64_from_le(&buf, 40 + j * 32 + 24);
                if b != 0 {
                    live.insert(b);
                }
            }
        }
        for p in blocks {
            if live.contains(&p) {
                return Err(invalid("queue aliases live allocator"));
            }
            ip_clear(dev, sm, bitmaps, p, bsz)?;
        }
        eph.retain(|e| e.oid == root || !seen.contains(&e.oid));
        eph.iter_mut()
            .find(|e| e.oid == root)
            .ok_or_else(|| invalid("queue root missing"))?
            .raw = crate::sm_fq::build_empty_sm_fq_node(root, xid, bsz);
        write_u64_le(&mut sm.raw, 200, 0);
        write_u64_le(&mut sm.raw, 216, 0);
    }
    let mut copies = Vec::new();
    let mut retired = HashSet::new();
    let mut old_cibs: Vec<_> = cibs.keys().copied().collect();
    old_cibs.sort_unstable();
    for old in old_cibs {
        let mut raw = cibs
            .remove(&old)
            .ok_or_else(|| invalid("missing dirty CIB"))?;
        let n = u32_from_le(&raw, 36) as usize;
        if 40 + n * 32 > bsz {
            return Err(invalid("dirty CIB bounds"));
        }
        for i in 0..n {
            let at = 40 + i * 32;
            let old_bm = u64_from_le(&raw, at + 24);
            if let Some(bytes) = bitmaps.remove(&old_bm) {
                let new = allocate_internal_pool_bitmap(dev, sm, bitmaps, bsz)?;
                copies.push((new, bytes));
                retired.insert(old_bm);
                write_u64_le(&mut raw, at + 24, new);
                write_u64_le(&mut raw, at, xid);
            }
        }
        let new = allocate_internal_pool_bitmap(dev, sm, bitmaps, bsz)?;
        let idx = (0..count)
            .find(|i| u64_from_le(&sm.raw, table + i * 8) == old)
            .ok_or_else(|| invalid("dirty CIB absent from table"))?;
        write_u64_le(&mut sm.raw, table + idx * 8, new);
        write_u64_le(&mut raw, 8, new);
        write_u64_le(&mut raw, 16, xid);
        update_checksum_in_place(&mut raw);
        copies.push((new, raw));
        retired.insert(old);
    }
    // Allocation is finished before any old live pool address becomes reusable.
    for old in retired {
        ip_clear(dev, sm, bitmaps, old, bsz)?;
    }
    let maps = u32_from_le(&sm.raw, 160) as usize;
    let slots = u32_from_le(&sm.raw, 164) as usize;
    let base = u64_from_le(&sm.raw, 168);
    let offsets = u32_from_le(&sm.raw, 328) as usize;
    let next = u32_from_le(&sm.raw, 332) as usize;
    let xids = u32_from_le(&sm.raw, 324) as usize;
    if slots == 0
        || slots > 65534
        || next + slots * 2 > sm.raw.len()
        || offsets + maps * 2 > sm.raw.len()
        || xids + maps * 8 > sm.raw.len()
    {
        return Err(invalid("pool rotation table bounds"));
    }
    for i in 0..maps {
        let old = u16_from_le(&sm.raw, offsets + i * 2) as usize;
        let Some(bytes) = bitmaps.remove(&(base + old as u64)) else {
            continue;
        };
        let head = u16_from_le(&sm.raw, 320) as usize;
        let tail = u16_from_le(&sm.raw, 322) as usize;
        if old >= slots || head >= slots || tail >= slots || head == old {
            return Err(invalid("invalid pool rotation"));
        }
        let successor = u16_from_le(&sm.raw, next + head * 2);
        if successor as usize >= slots {
            return Err(invalid("pool bitmap reserve exhausted"));
        }
        put16(&mut sm.raw, 320, successor)?;
        put16(&mut sm.raw, next + head * 2, 65535)?;
        put16(&mut sm.raw, next + tail * 2, old as u16)?;
        put16(&mut sm.raw, next + old * 2, 65535)?;
        put16(&mut sm.raw, 322, old as u16)?;
        put16(&mut sm.raw, offsets + i * 2, head as u16)?;
        write_u64_le(&mut sm.raw, xids + i * 8, xid);
        copies.push((base + head as u64, bytes));
    }
    if !bitmaps.is_empty() {
        return Err(invalid("unaccounted dirty allocation bitmap"));
    }
    for (p, bytes) in copies {
        dev.write_at(p * bsz as u64, &bytes)?;
    }
    Ok(())
}
