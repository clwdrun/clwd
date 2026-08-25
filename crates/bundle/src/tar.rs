//! Canonical USTAR, both directions. See docs/spec/bundle-v1.md §3.
//!
//! The `tar` crate is deliberately not used here. The decoder must *validate*
//! bytes rather than extract them — a permissive reader accepts archives this
//! format rejects — and the encoder must control every byte, including a
//! checksum encoding the crate does not emit. That leaves one 512-byte field
//! layout shared by both directions, which is less code than adapting a crate
//! on one side and hand-writing the other.

use crate::error::Error;

pub const BLOCK: usize = 512;

// Field offsets, §3.4.
const NAME: (usize, usize) = (0, 100);
const MODE: (usize, usize) = (100, 8);
const UID: (usize, usize) = (108, 8);
const GID: (usize, usize) = (116, 8);
const SIZE: (usize, usize) = (124, 12);
const MTIME: (usize, usize) = (136, 12);
const CKSUM: (usize, usize) = (148, 8);
const TYPEFLAG: usize = 156;
const LINKNAME: (usize, usize) = (157, 100);
const MAGIC: (usize, usize) = (257, 6);
const VERSION: (usize, usize) = (263, 2);
const UNAME: (usize, usize) = (265, 32);
const GNAME: (usize, usize) = (297, 32);
const DEVMAJOR: (usize, usize) = (329, 8);
const DEVMINOR: (usize, usize) = (337, 8);
const PREFIX: (usize, usize) = (345, 155);
const TAIL: (usize, usize) = (500, 12);

const REGULAR: u8 = 0x30;
const MODE_FILE: &[u8] = b"0000644\0";
const MODE_EXEC: &[u8] = b"0000755\0";
const ZERO_NUMERIC: &[u8] = b"0000000\0";
const ZERO_MTIME: &[u8] = b"00000000000\0";

/// One member as it appears on the wire: a validated path, its mode, and its
/// contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub executable: bool,
    pub data: Vec<u8>,
}

fn field<'a>(block: &'a [u8], (at, len): (usize, usize)) -> &'a [u8] {
    &block[at..at + len]
}

/// The §3.4.2 checksum: every header byte summed with the checksum field itself
/// read as eight ASCII spaces.
fn checksum(block: &[u8]) -> u32 {
    block
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if (CKSUM.0..CKSUM.0 + CKSUM.1).contains(&i) {
                u32::from(b' ')
            } else {
                u32::from(*b)
            }
        })
        .sum()
}

/// Build one canonical header block.
pub fn write_header(path: &str, len: u64, executable: bool) -> [u8; BLOCK] {
    let mut block = [0u8; BLOCK];
    block[NAME.0..NAME.0 + path.len()].copy_from_slice(path.as_bytes());
    block[MODE.0..MODE.0 + MODE.1]
        .copy_from_slice(if executable { MODE_EXEC } else { MODE_FILE });
    block[UID.0..UID.0 + UID.1].copy_from_slice(ZERO_NUMERIC);
    block[GID.0..GID.0 + GID.1].copy_from_slice(ZERO_NUMERIC);
    block[SIZE.0..SIZE.0 + SIZE.1].copy_from_slice(format!("{len:011o}\0").as_bytes());
    block[MTIME.0..MTIME.0 + MTIME.1].copy_from_slice(ZERO_MTIME);
    block[TYPEFLAG] = REGULAR;
    block[MAGIC.0..MAGIC.0 + MAGIC.1].copy_from_slice(b"ustar\0");
    block[VERSION.0..VERSION.0 + VERSION.1].copy_from_slice(b"00");

    // Six octal digits, NUL, space — §3.4.2. Written last: it covers the rest.
    let sum = checksum(&block);
    block[CKSUM.0..CKSUM.0 + CKSUM.1].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    block
}

/// Parse and validate a canonical tar stream, in the order §8 requires.
///
/// This does not extract permissively and check afterwards: every field is
/// compared against the one value the format permits, so an archive that would
/// decode to the right content but differ in any byte is rejected here.
pub fn read(stream: &[u8]) -> Result<Vec<Entry>, Error> {
    if stream.is_empty() || stream.len() % BLOCK != 0 {
        return Err(Error::NotTar {
            reason: "length is zero or not a multiple of 512",
        });
    }

    let mut entries: Vec<Entry> = Vec::new();
    let mut offset = 0usize;

    while offset + BLOCK <= stream.len() {
        let block = &stream[offset..offset + BLOCK];

        if block.iter().all(|b| *b == 0) {
            return finish(stream, offset, entries);
        }

        // Step 2: header fields, in the order the specification lists them.
        if field(block, MAGIC) != b"ustar\0" || field(block, VERSION) != b"00" {
            return Err(Error::MagicInvalid { offset });
        }
        if block[TYPEFLAG] != REGULAR {
            return Err(Error::EntryTypeInvalid {
                offset,
                typeflag: block[TYPEFLAG],
            });
        }
        if field(block, PREFIX).iter().any(|b| *b != 0) {
            return Err(Error::PrefixNotEmpty { offset });
        }
        let mode = field(block, MODE);
        if mode != MODE_FILE && mode != MODE_EXEC {
            return Err(Error::ModeInvalid {
                offset,
                mode: String::from_utf8_lossy(mode).trim_end_matches('\0').to_owned(),
            });
        }
        for (name, got, want) in [
            ("uid", field(block, UID), ZERO_NUMERIC),
            ("gid", field(block, GID), ZERO_NUMERIC),
            ("mtime", field(block, MTIME), ZERO_MTIME),
        ] {
            if got != want {
                return Err(Error::MetadataNotCanonical { offset, field: name });
            }
        }
        for (name, span) in [
            ("uname", UNAME),
            ("gname", GNAME),
            ("linkname", LINKNAME),
            ("devmajor", DEVMAJOR),
            ("devminor", DEVMINOR),
            ("header tail", TAIL),
        ] {
            if field(block, span).iter().any(|b| *b != 0) {
                return Err(Error::MetadataNotCanonical { offset, field: name });
            }
        }

        // Step 3: size encoding, then its ceiling.
        let size = parse_size(field(block, SIZE), offset)?;

        // Step 4: checksum value and encoding.
        let expected = format!("{:06o}\0 ", checksum(block));
        if field(block, CKSUM) != expected.as_bytes() {
            return Err(Error::ChecksumInvalid { offset });
        }

        // Step 5: the path.
        let path = read_path(field(block, NAME))?;
        crate::path::validate(&path)?;
        if entries.iter().any(|e| e.path == path) {
            return Err(Error::PathDuplicate { path });
        }

        // Step 6: data and its NUL padding.
        let start = offset + BLOCK;
        let blocks = (size as usize).div_ceil(BLOCK);
        let end = start + blocks * BLOCK;
        if end > stream.len() {
            return Err(Error::NotTar {
                reason: "stream ends inside an entry",
            });
        }
        let data = stream[start..start + size as usize].to_vec();
        if let Some(pad) = stream[start + size as usize..end].iter().position(|b| *b != 0) {
            return Err(Error::PaddingInvalid {
                offset: start + size as usize + pad,
            });
        }

        // Step 7: ordering, checked against the entry just before this one.
        if let Some(previous) = entries.last() {
            if path.as_bytes() <= previous.path.as_bytes() {
                return Err(Error::EntriesNotSorted {
                    previous: previous.path.clone(),
                    current: path,
                });
            }
        }

        entries.push(Entry {
            path,
            executable: mode == MODE_EXEC,
            data,
        });
        offset = end;
    }

    Err(Error::TrailerInvalid {
        reason: "stream ends without the two zero blocks",
    })
}

/// Step 8: the trailer is exactly two zero blocks, with nothing after them.
fn finish(stream: &[u8], offset: usize, entries: Vec<Entry>) -> Result<Vec<Entry>, Error> {
    let rest = &stream[offset..];
    if rest.len() < 2 * BLOCK {
        return Err(Error::TrailerInvalid {
            reason: "fewer than two trailing zero blocks",
        });
    }
    if rest.len() > 2 * BLOCK {
        // Includes the blocking-factor padding GNU tar emits by default.
        return Err(Error::TrailerInvalid {
            reason: "bytes follow the two trailing zero blocks",
        });
    }
    if rest.iter().any(|b| *b != 0) {
        return Err(Error::TrailerInvalid {
            reason: "trailing blocks are not all zero",
        });
    }
    Ok(entries)
}

/// §3.4.3: 11 octal digits then NUL. Base-256 is a GNU extension and is not
/// decoded, only rejected — its high-bit signal is not an octal digit.
fn parse_size(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    if bytes[11] != 0 || bytes[..11].iter().any(|b| !(b'0'..=b'7').contains(b)) {
        return Err(Error::SizeEncodingInvalid { offset });
    }
    let mut size: u64 = 0;
    for byte in &bytes[..11] {
        size = size * 8 + u64::from(byte - b'0');
    }
    if size >= 8 * 1024 * 1024 * 1024 {
        return Err(Error::MemberTooLarge { size });
    }
    Ok(size)
}

fn read_path(name: &[u8]) -> Result<String, Error> {
    let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
    String::from_utf8(name[..end].to_vec()).map_err(|_| Error::PathInvalid {
        path: String::from_utf8_lossy(&name[..end]).into_owned(),
        reason: "not valid UTF-8",
    })
}
