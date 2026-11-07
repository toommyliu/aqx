use std::io::{self, Read};

const MAX_BYTES: u64 = 32 * 1024 * 1024;

pub(crate) fn labels(bytes: &[u8]) -> io::Result<Vec<String>> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid map file");
    let header = bytes.get(..8).ok_or_else(invalid)?;
    let body = match &header[..3] {
        b"FWS" => bytes[8..].to_vec(),
        b"CWS" => {
            let mut body = Vec::new();
            flate2::read::ZlibDecoder::new(&bytes[8..])
                .take(MAX_BYTES + 1)
                .read_to_end(&mut body)?;
            if body.len() as u64 > MAX_BYTES {
                return Err(invalid());
            }
            body
        }
        _ => return Err(invalid()),
    };
    let bits = body.first().ok_or_else(invalid)? >> 3;
    let mut at = (5 + 4 * bits as usize).div_ceil(8) + 4;
    let mut cells = Vec::new();
    loop {
        let tag = body.get(at..at + 2).ok_or_else(invalid)?;
        let tag = u16::from_le_bytes(tag.try_into().unwrap());
        at += 2;
        let kind = tag >> 6;
        let mut len = (tag & 63) as usize;
        if len == 63 {
            let size = body.get(at..at + 4).ok_or_else(invalid)?;
            len = u32::from_le_bytes(size.try_into().unwrap()) as usize;
            at += 4;
        }
        let data = body
            .get(at..at.checked_add(len).ok_or_else(invalid)?)
            .ok_or_else(invalid)?;
        at += len;
        if kind == 0 {
            return Ok(cells);
        }
        if kind == 43 {
            let end = data.iter().position(|b| *b == 0).ok_or_else(invalid)?;
            let label = std::str::from_utf8(&data[..end]).map_err(io::Error::other)?;
            if !label.is_empty() && !cells.iter().any(|s| s == label) {
                cells.push(label.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn map_cells_come_from_frame_labels() {
        let mut bytes = b"FWS\x0a\0\0\0\0\0\0\x18\x01\0".to_vec();
        for cell in ["Enter", "Side1", "Blank", "Enter"] {
            bytes.extend_from_slice(&((43 << 6) | (cell.len() + 1) as u16).to_le_bytes());
            bytes.extend_from_slice(cell.as_bytes());
            bytes.push(0);
        }
        bytes.extend_from_slice(&[0, 0]);
        assert_eq!(labels(&bytes).unwrap(), ["Enter", "Side1", "Blank"]);
        use std::io::Write;
        let mut encoded =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoded.write_all(&bytes[8..]).unwrap();
        let compressed = [b"CWS\x0a\0\0\0\0".as_slice(), &encoded.finish().unwrap()].concat();
        assert_eq!(labels(&compressed).unwrap(), ["Enter", "Side1", "Blank"]);
        assert!(labels(b"FWS").is_err());
        bytes.pop();
        assert!(labels(&bytes).is_err());
    }
}
