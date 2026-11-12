//! Just enough of the zip format to unpack a release package: the central directory, stored
//! (0) and deflated (8) entries, CRC-32 checked. No zip64, no encryption, no multi-disk.
//!
//! Only entries whose *file name* (the last path component) is one of the wanted names are
//! extracted, and always to `dest\<that name>`: the paths inside the archive never decide where
//! a file lands.

use std::io::Read;
use std::path::Path;

/// Largest uncompressed entry accepted.
const MAX_ENTRY: u64 = 256 * 1024 * 1024;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// One central-directory entry.
#[derive(Debug, Clone)]
struct Entry {
    name: String,
    method: u16,
    flags: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    local_offset: u64,
}

fn entries(zip: &[u8]) -> Result<Vec<Entry>, String> {
    const EOCD: u32 = 0x0605_4b50;
    const CEN: u32 = 0x0201_4b50;
    let bad = |what: &str| format!("not a valid zip ({what})");
    if zip.len() < 22 {
        return Err(bad("too short"));
    }
    // End of central directory: last 22 bytes + up to 64 KiB of comment.
    let floor = zip.len().saturating_sub(22 + 0xFFFF);
    let eocd = (floor..=zip.len() - 22)
        .rev()
        .find(|&i| u32_at(zip, i) == Some(EOCD))
        .ok_or_else(|| bad("no end record"))?;
    let count = u16_at(zip, eocd + 10).ok_or_else(|| bad("end record"))? as usize;
    let cd_size = u32_at(zip, eocd + 12).ok_or_else(|| bad("end record"))? as usize;
    let cd_off = u32_at(zip, eocd + 16).ok_or_else(|| bad("end record"))? as usize;
    if cd_off == 0xFFFF_FFFF || count == 0xFFFF {
        return Err(bad("zip64"));
    }
    if cd_off.checked_add(cd_size).is_none_or(|end| end > eocd) {
        return Err(bad("central directory"));
    }
    let mut out = Vec::with_capacity(count);
    let mut at = cd_off;
    for _ in 0..count {
        if u32_at(zip, at) != Some(CEN) {
            return Err(bad("central directory entry"));
        }
        let field = |o: usize| u16_at(zip, at + o).ok_or_else(|| bad("entry"));
        let field32 = |o: usize| u32_at(zip, at + o).ok_or_else(|| bad("entry"));
        let flags = field(8)?;
        let method = field(10)?;
        let crc = field32(16)?;
        let compressed = field32(20)?;
        let size = field32(24)?;
        let name_len = field(28)? as usize;
        let extra_len = field(30)? as usize;
        let comment_len = field(32)? as usize;
        let local = field32(42)?;
        let name_bytes = zip
            .get(at + 46..at + 46 + name_len)
            .ok_or_else(|| bad("entry name"))?;
        if compressed == 0xFFFF_FFFF || size == 0xFFFF_FFFF || local == 0xFFFF_FFFF {
            return Err(bad("zip64"));
        }
        out.push(Entry {
            name: String::from_utf8_lossy(name_bytes).into_owned(),
            method,
            flags,
            crc,
            compressed: u64::from(compressed),
            size: u64::from(size),
            local_offset: u64::from(local),
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

/// The data of `entry`, uncompressed and CRC-checked.
fn read_entry(zip: &[u8], entry: &Entry) -> Result<Vec<u8>, String> {
    const LOC: u32 = 0x0403_4b50;
    let bad = |what: &str| format!("not a valid zip ({what}: {})", entry.name);
    if entry.flags & 1 != 0 {
        return Err(bad("encrypted"));
    }
    if entry.size > MAX_ENTRY {
        return Err(bad("too large"));
    }
    let at = usize::try_from(entry.local_offset).map_err(|_| bad("offset"))?;
    if u32_at(zip, at) != Some(LOC) {
        return Err(bad("local header"));
    }
    let name_len = u16_at(zip, at + 26).ok_or_else(|| bad("local header"))? as usize;
    let extra_len = u16_at(zip, at + 28).ok_or_else(|| bad("local header"))? as usize;
    let start = at + 30 + name_len + extra_len;
    let len = usize::try_from(entry.compressed).map_err(|_| bad("size"))?;
    let data = zip
        .get(start..start + len)
        .ok_or_else(|| bad("truncated"))?;
    let out = match entry.method {
        0 => data.to_vec(),
        8 => {
            let mut out = Vec::with_capacity(entry.size as usize);
            flate2::read::DeflateDecoder::new(data)
                .take(entry.size + 1)
                .read_to_end(&mut out)
                .map_err(|_| bad("deflate"))?;
            out
        }
        m => return Err(bad(&format!("compression method {m}"))),
    };
    if out.len() as u64 != entry.size {
        return Err(bad("size mismatch"));
    }
    if crc32fast::hash(&out) != entry.crc {
        return Err(bad("CRC mismatch"));
    }
    Ok(out)
}

/// Last path component of an archive name (`/` or `\` separated).
fn file_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// Extract every entry whose file name is in `wanted` (case-insensitive) to `dest\<name>`.
/// Each wanted name may appear once at most. Returns the names extracted.
pub fn extract_named(zip: &[u8], wanted: &[&str], dest: &Path) -> Result<Vec<String>, String> {
    let all = entries(zip)?;
    let mut done: Vec<String> = Vec::new();
    for entry in &all {
        if entry.name.ends_with('/') {
            continue;
        }
        let base = file_name(&entry.name);
        let Some(target) = wanted.iter().find(|w| w.eq_ignore_ascii_case(base)) else {
            continue;
        };
        if done.iter().any(|d| d.eq_ignore_ascii_case(target)) {
            return Err(format!("not a valid package (two {target})"));
        }
        let data = read_entry(zip, entry)?;
        std::fs::create_dir_all(dest).map_err(|e| format!("cannot unpack: {}", e.kind()))?;
        std::fs::write(dest.join(target), &data)
            .map_err(|e| format!("cannot unpack {target}: {}", e.kind()))?;
        done.push(target.to_string());
    }
    Ok(done)
}

/// Test helper: a zip with the given `(name, data, deflate)` entries.
#[cfg(test)]
pub(crate) fn build(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    use std::io::Write;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, deflate) in entries {
        let (method, body) = if *deflate {
            let mut enc =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
            enc.write_all(data).unwrap();
            (8u16, enc.finish().unwrap())
        } else {
            (0u16, data.to_vec())
        };
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let mut local = Vec::new();
        local.extend(0x0403_4b50u32.to_le_bytes());
        local.extend(20u16.to_le_bytes());
        local.extend(0u16.to_le_bytes());
        local.extend(method.to_le_bytes());
        local.extend([0u8; 4]);
        local.extend(crc.to_le_bytes());
        local.extend((body.len() as u32).to_le_bytes());
        local.extend((data.len() as u32).to_le_bytes());
        local.extend((name.len() as u16).to_le_bytes());
        local.extend(0u16.to_le_bytes());
        local.extend(name.as_bytes());
        out.extend(local);
        out.extend(&body);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(method.to_le_bytes());
        central.extend([0u8; 4]);
        central.extend(crc.to_le_bytes());
        central.extend((body.len() as u32).to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((name.len() as u16).to_le_bytes());
        central.extend([0u8; 12]);
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let cd_off = out.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(cd_off.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_wanted_names_only_into_dest() {
        let big = vec![b'M'; 100_000];
        let zip = build(&[
            ("CodeNotch-1.1.0/", b"", false),
            ("CodeNotch-1.1.0/codenotch.exe", &big, true),
            ("CodeNotch-1.1.0/codenotch-hook.exe", b"MZhook", false),
            ("CodeNotch-1.1.0/README.md", b"readme", true),
            ("../../evil/CODENOTCH.EXE.txt", b"x", false),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let done =
            extract_named(&zip, &["codenotch.exe", "codenotch-hook.exe"], dir.path()).unwrap();
        assert_eq!(done, ["codenotch.exe", "codenotch-hook.exe"]);
        assert_eq!(
            std::fs::read(dir.path().join("codenotch.exe")).unwrap(),
            big
        );
        assert_eq!(
            std::fs::read(dir.path().join("codenotch-hook.exe")).unwrap(),
            b"MZhook"
        );
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn traversal_names_still_land_in_dest() {
        let zip = build(&[("..\\..\\Windows\\codenotch.exe", b"MZ", false)]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out");
        assert_eq!(
            extract_named(&zip, &["codenotch.exe"], &dest).unwrap(),
            ["codenotch.exe"]
        );
        assert!(dest.join("codenotch.exe").is_file());
    }

    #[test]
    fn rejects_corruption_and_duplicates() {
        let mut zip = build(&[("a/codenotch.exe", b"MZ data here", false)]);
        // Flip a data byte: CRC mismatch.
        let pos = zip.windows(4).position(|w| w == b"data").unwrap();
        zip[pos] = b'X';
        let dir = tempfile::tempdir().unwrap();
        assert!(extract_named(&zip, &["codenotch.exe"], dir.path())
            .unwrap_err()
            .contains("CRC"));
        let dup = build(&[
            ("a/codenotch.exe", b"1", false),
            ("b/codenotch.exe", b"2", false),
        ]);
        assert!(extract_named(&dup, &["codenotch.exe"], dir.path()).is_err());
        assert!(extract_named(b"PK not a zip at all......", &["x"], dir.path()).is_err());
        assert!(extract_named(&[], &["x"], dir.path()).is_err());
        // Truncated archive.
        let ok = build(&[("codenotch.exe", b"MZ", true)]);
        assert!(extract_named(&ok[..ok.len() - 5], &["codenotch.exe"], dir.path()).is_err());
    }
}
