//! Title code sites that write bytes already there — patched at load to check first
//! (`XCL_WRITE_SITES=<file>`; off unless set): `or byte` flag updates, register moves, and
//! selected SIMD stores (CGBJ `titlewrite` / loader `write_sites`, reimplemented).

use std::path::Path;

use super::patch_insn;

fn or_byte(code: &[u8]) -> Option<(usize, u8, u8, &[u8], u8)> {
    let mut at = 0;
    let mut rex = 0;
    if code.first()? & 0xf0 == 0x40 {
        rex = code[0];
        at = 1;
    }
    let (&op, &modrm) = (code.get(at)?, code.get(at + 1)?);
    if op != 0x80 || modrm >> 3 & 7 != 1 {
        return None;
    }
    let (mode, rm) = (modrm >> 6, modrm & 7);
    if mode == 3 || mode == 0 && rm == 5 {
        return None;
    }
    let rest = at + 2;
    let mut end = rest;
    if rm == 4 {
        let sib = *code.get(end)?;
        if sib & 7 == 4 && rex & 1 == 0 || sib & 7 == 5 && mode == 0 {
            return None;
        }
        end += 1;
    }
    end += match mode {
        1 => 1,
        2 => 4,
        _ => 0,
    };
    let constant = *code.get(end)?;
    Some((end + 1, rex, modrm, &code[rest..end], constant))
}

fn address(code: &[u8], modrm: u8, b: bool) -> Option<usize> {
    let (mode, rm) = (modrm >> 6, modrm & 7);
    if mode == 3 || mode == 0 && rm == 5 {
        return None;
    }
    let mut len = 0;
    if rm == 4 {
        let sib = *code.first()?;
        if sib & 7 == 4 && !b || sib & 7 == 5 && mode == 0 {
            return None;
        }
        len = 1;
    }
    len += match mode {
        1 => 1,
        2 => 4,
        _ => 0,
    };
    Some(len)
}

fn write_or_not(out: &mut Vec<u8>, stub: usize, code: &[u8], at: usize, len: usize, wide: bool) -> Option<()> {
    if !wide {
        out.push(0x67);
    }
    out.extend([0xe3, u8::try_from(1 + len + 5).ok()?]);
    out.push(0x59);
    out.extend(&code[..len]);
    out.extend(patch_insn::jump(stub + out.len(), at + len)?);
    out.push(0x59);
    out.extend(patch_insn::jump(stub + out.len(), at + len)?);
    Some(())
}

fn vector_stub(code: &[u8], at: usize, stub: usize) -> Option<(usize, Vec<u8>)> {
    let (reg_high, xb, third, op_at) = match *code.first()? {
        0xc5 => {
            let b = *code.get(1)?;
            (b & 0x80 == 0, 0x60, b, 2)
        }
        0xc4 => {
            let b = *code.get(1)?;
            if b & 0x1f != 1 {
                return None;
            }
            (b & 0x80 == 0, b & 0x60, *code.get(2)?, 3)
        }
        _ => return None,
    };
    let (op, modrm) = (*code.get(op_at)?, *code.get(op_at + 1)?);
    if !matches!((op, third & 3), (0x11 | 0x29, 0 | 1) | (0x7f, 1 | 2)) || third >> 3 & 15 != 15 {
        return None;
    }
    let long = third & 4;
    if !std::is_x86_feature_detected!("avx2") {
        return None;
    }
    let rest = address(code.get(op_at + 2..)?, modrm, xb & 0x20 == 0)?;
    let len = op_at + 2 + rest;
    if len < 5 {
        return None;
    }
    let reg = modrm >> 3 & 7 | if reg_high { 8 } else { 0 };
    let spare = if reg == 1 { 2u8 } else { 1 };
    let mut out = vec![0x51];
    out.extend([0x48, 0x8d, 0x64, 0x24, 0xe0]);
    out.extend([0xc5, 0xfe, 0x7f, spare << 3 | 4, 0x24]);
    out.extend([0xc4, 0x80 | xb | 1, (!reg & 15) << 3 | long | 1, 0x74, modrm & 0xc7 | spare << 3]);
    out.extend(&code[op_at + 2..len]);
    out.extend([0xc5, 0xf9 | long, 0xd7, 0xc8 | spare]);
    out.extend([0xc5, 0xfe, 0x6f, spare << 3 | 4, 0x24]);
    out.extend([0x48, 0x8d, 0x64, 0x24, 0x20]);
    out.extend([0xf7, 0xd1]);
    if long == 0 {
        out.extend([0x0f, 0xb7, 0xc9]);
    }
    write_or_not(&mut out, stub, code, at, len, false)?;
    Some((len, out))
}

fn move_stub(code: &[u8], at: usize, stub: usize) -> Option<(usize, Vec<u8>)> {
    let mut at_op = 0;
    let mut rex = 0x40;
    if code.first()? & 0xf0 == 0x40 {
        rex = code[0];
        at_op = 1;
    }
    let (op, modrm) = (*code.get(at_op)?, *code.get(at_op + 1)?);
    let wide = rex & 8 != 0;
    let reg = modrm >> 3 & 7 | (rex & 4) << 1;
    let rest = address(code.get(at_op + 2..)?, modrm, rex & 1 != 0)?;
    let end = at_op + 2 + rest;
    let (len, constant) = match op {
        0x89 if reg != 1 && reg != 4 => (end, None),
        0xc7 if reg & 7 == 0 => {
            let bytes = code.get(end..end + 4)?.try_into().ok()?;
            (end + 4, Some(i32::from_le_bytes(bytes).checked_add(1)?))
        }
        _ => return None,
    };
    if len < 5 {
        return None;
    }
    let mut out = vec![0x51];
    out.extend([rex & 0x4b, 0x8b, modrm & 0xc7 | 1 << 3]);
    out.extend(&code[at_op + 2..end]);
    let w = 0x40 | rex & 8;
    out.extend([w, 0xf7, 0xd1]);
    match constant {
        Some(next) => {
            out.extend([w, 0x8d, 0x89]);
            out.extend(next.to_le_bytes());
        }
        None => out.extend([w | (reg & 8) >> 2, 0x8d, 0x4c, (reg & 7) << 3 | 1, 1]),
    }
    write_or_not(&mut out, stub, code, at, len, wide)?;
    Some((len, out))
}

fn or_stub(code: &[u8], at: usize, stub: usize) -> Option<(usize, Vec<u8>)> {
    let (len, rex, modrm, rest, constant) = or_byte(code)?;
    if len < 5 {
        return None;
    }
    let mut out = vec![0x50, 0x51];
    if rex & 0x43 != 0 {
        out.push(rex & 0x43);
    }
    out.extend([0x0f, 0xb6, modrm & 0xc7]);
    out.extend(rest);
    out.extend([0x89, 0xc1]);
    out.extend([0xf7, 0xd1]);
    out.extend([0x81, 0xe1, constant, 0, 0, 0]);
    out.extend([0x0c, constant]);
    let back = |out: &mut Vec<u8>| {
        let from = stub + out.len();
        out.extend(patch_insn::jump(from, at + len)?);
        Some(())
    };
    let mut write = vec![0x59, 0x58];
    write.extend(&code[..len]);
    out.extend([0x67, 0xe3, u8::try_from(write.len() + 5).ok()?]);
    out.extend(write);
    back(&mut out)?;
    out.extend([0x59, 0x58]);
    back(&mut out)?;
    Some((len, out))
}

fn stub(code: &[u8], at: usize, stub: usize) -> Option<(usize, Vec<u8>)> {
    vector_stub(code, at, stub).or_else(|| move_stub(code, at, stub)).or_else(|| or_stub(code, at, stub))
}

/// Patch sites named in `list` for image `name` mapped at `base` (`size` bytes). Returns count patched.
///
/// # Safety
/// The image stays mapped and no thread has entered its code yet.
pub unsafe fn apply(name: &str, base: usize, size: usize, list: &str) -> usize {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut room = None::<(usize, usize)>;
    let mut done = 0;
    let mut seen = std::collections::BTreeSet::new();
    for line in list.lines() {
        let mut words = line.split_whitespace();
        let (Some(image), Some(offset)) = (words.next(), words.next()) else {
            continue;
        };
        let Ok(offset) = usize::from_str_radix(offset.trim_start_matches("0x"), 16) else {
            continue;
        };
        if !image.eq_ignore_ascii_case(name) || offset + 16 > size || !seen.insert(offset) {
            continue;
        }
        let at = base + offset;
        if room.is_none() {
            room = patch_insn::stub_room(at);
        }
        let Some((place, used)) = room.as_mut() else {
            return done;
        };
        let code: [u8; 16] = unsafe { std::ptr::read_unaligned(at as *const _) };
        let to = *place + *used;
        let Some((len, body)) = stub(&code, at, to) else {
            crate::xlog_warn!("loader: write site at {offset:#x} is not a patchable store/or");
            continue;
        };
        let Some(jump) = patch_insn::jump(at, to) else {
            continue;
        };
        if !patch_insn::bump_stub_used(used, body.len()) {
            return done;
        }
        let mut old = 0;
        unsafe {
            std::ptr::copy_nonoverlapping(body.as_ptr(), to as *mut u8, body.len());
            if VirtualProtect(at as *const _, len, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                continue;
            }
            std::ptr::write_bytes(at as *mut u8, 0x90, len);
            std::ptr::copy_nonoverlapping(jump.as_ptr(), at as *mut u8, jump.len());
            VirtualProtect(at as *const _, len, old, &mut old);
            FlushInstructionCache(GetCurrentProcess(), at as *const _, len);
        }
        done += 1;
    }
    done
}

/// [`apply`] with the path from `XCL_WRITE_SITES`.
///
/// # Safety
/// As [`apply`].
pub unsafe fn apply_asked(name: &str, base: usize, size: usize) {
    let Some(file) = std::env::var_os("XCL_WRITE_SITES") else {
        return;
    };
    let Ok(list) = std::fs::read_to_string(Path::new(&file)) else {
        crate::xlog_warn!("loader: could not read XCL_WRITE_SITES file {:?}", file);
        return;
    };
    let places = unsafe { apply(name, base, size, &list) };
    if places > 0 {
        crate::xlog_info!("loader: {places} title write sites look-first (XCL_WRITE_SITES)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn or_byte_decodes_title_flag_set() {
        let (len, rex, modrm, rest, constant) = or_byte(&[0x41, 0x80, 0x89, 0x9e, 0, 0, 0, 1, 0, 0]).unwrap();
        assert_eq!((len, rex, modrm, rest, constant), (8, 0x41, 0x89, &[0x9e, 0, 0, 0][..], 1));
    }
}