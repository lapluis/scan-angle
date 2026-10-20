//! Metadata-only frame discovery for APNG default images and MPF JPEGs.
use anyhow::{Result, bail};
use std::io::{Read, Seek, SeekFrom};

pub fn apng_default<R: Read + Seek>(reader: &mut R) -> Result<bool> {
    reader.seek(SeekFrom::Start(8))?;
    let mut animated = false;
    let result = (|| {
        loop {
            let mut head = [0; 8];
            reader.read_exact(&mut head)?;
            let n = u32::from_be_bytes(head[..4].try_into().unwrap());
            match &head[4..] {
                b"acTL" => animated = true,
                b"fcTL" => return Ok(false),
                b"IDAT" => return Ok(animated),
                b"IEND" => return Ok(false),
                _ => {}
            }
            reader.seek(SeekFrom::Current(i64::from(n) + 4))?;
        }
    })();
    reader.seek(SeekFrom::Start(0))?;
    result
}
pub fn mpo_entries<R: Read + Seek>(reader: &mut R) -> Result<Vec<(u64, u64)>> {
    let length = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(2))?;
    let result = (|| {
        loop {
            let mut marker = [0; 2];
            reader.read_exact(&mut marker)?;
            if marker[0] != 0xff {
                bail!("无效 JPEG 标记");
            }
            while marker[1] == 0xff {
                reader.read_exact(&mut marker[1..])?;
            }
            if marker[1] == 0xda || marker[1] == 0xd9 {
                return Ok(Vec::new());
            }
            if marker[1] == 0x01 || (0xd0..=0xd7).contains(&marker[1]) {
                continue;
            }
            let mut len = [0; 2];
            reader.read_exact(&mut len)?;
            let size = u16::from_be_bytes(len) as usize;
            if size < 2 {
                bail!("无效 JPEG 段长度");
            }
            let start = reader.stream_position()?;
            if marker[1] == 0xe2 {
                let mut payload = vec![0; size - 2];
                reader.read_exact(&mut payload)?;
                if payload.starts_with(b"MPF\0") {
                    return parse_mpf(&payload[4..], start + 4, length);
                }
            } else {
                reader.seek(SeekFrom::Current((size - 2) as i64))?;
            }
        }
    })();
    reader.seek(SeekFrom::Start(0))?;
    result
}
fn parse_mpf(data: &[u8], base: u64, file_len: u64) -> Result<Vec<(u64, u64)>> {
    let little = match data.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => bail!("无效 MPF 字节序"),
    };
    let u16_at = |i: usize| -> Result<u16> {
        let b: [u8; 2] = data
            .get(i..i + 2)
            .ok_or_else(|| anyhow::anyhow!("截断 MPF"))?
            .try_into()?;
        Ok(if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |i: usize| -> Result<u32> {
        let b: [u8; 4] = data
            .get(i..i + 4)
            .ok_or_else(|| anyhow::anyhow!("截断 MPF"))?
            .try_into()?;
        Ok(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    for i in 0..count {
        let at = ifd + 2 + i * 12;
        if u16_at(at)? != 0xb002 {
            continue;
        }
        let bytes = u32_at(at + 4)? as usize;
        let offset = u32_at(at + 8)? as usize;
        if !bytes.is_multiple_of(16)
            || bytes == 0
            || offset.checked_add(bytes).is_none_or(|n| n > data.len())
        {
            bail!("无效 MPF 图像目录");
        }
        let mut entries = Vec::new();
        for n in 0..bytes / 16 {
            let size = u32_at(offset + n * 16 + 4)? as u64;
            let start = if n == 0 {
                0
            } else {
                base + u32_at(offset + n * 16 + 8)? as u64
            };
            if size == 0 || start.checked_add(size).is_none_or(|end| end > file_len) {
                bail!("MPF 图像偏移越界");
            }
            entries.push((start, size));
        }
        return Ok(entries);
    }
    Ok(Vec::new())
}
