use std::io::Read;

const END: u16 = 0;
const FRAME_LABEL: u16 = 43;
const SCENE_AND_FRAME_LABELS: u16 = 86;

pub(crate) fn frame_labels(swf: &[u8]) -> Result<Vec<String>, String> {
    let rest = swf.get(8..).ok_or("the file is too short to be a SWF")?;
    let body = match &swf[..3] {
        b"FWS" => rest.to_vec(),
        b"CWS" => {
            let mut body = Vec::new();
            flate2::read::ZlibDecoder::new(rest).read_to_end(&mut body).map_err(|e| format!("the SWF won't inflate: {e}"))?;
            body
        }
        b"ZWS" => return Err("the SWF is LZMA-compressed, which aqx doesn't read".into()),
        _ => return Err("the file isn't a SWF".into()),
    };
    let nbits = usize::from(*body.first().ok_or("the SWF has no header")? >> 3);
    let mut at = (5 + 4 * nbits).div_ceil(8) + 4;
    let (mut scene, mut frames) = (None, Vec::new());
    while let Some(&[low, high]) = body.get(at..at + 2) {
        at += 2;
        let header = u16::from_le_bytes([low, high]);
        let mut len = usize::from(header & 0x3f);
        if len == 0x3f {
            let long = body.get(at..at + 4).ok_or("a SWF tag is cut off")?;
            len = u32::from_le_bytes([long[0], long[1], long[2], long[3]]) as usize;
            at += 4;
        }
        let tag = body.get(at..at + len).ok_or("a SWF tag is cut off")?;
        at += len;
        match header >> 6 {
            END => break,
            FRAME_LABEL => frames.push(Reader(tag, 0).string()?),
            SCENE_AND_FRAME_LABELS => scene = Some(scene_labels(tag)?),
            _ => {}
        }
    }
    let mut labels = scene.unwrap_or(frames);
    let mut seen = std::collections::HashSet::new();
    labels.retain(|label| seen.insert(label.clone()));
    Ok(labels)
}

fn scene_labels(tag: &[u8]) -> Result<Vec<String>, String> {
    let mut read = Reader(tag, 0);
    for _ in 0..read.encoded_u32()? {
        read.encoded_u32()?;
        read.string()?;
    }
    (0..read.encoded_u32()?)
        .map(|_| {
            read.encoded_u32()?;
            read.string()
        })
        .collect()
}

struct Reader<'a>(&'a [u8], usize);

impl Reader<'_> {
    fn encoded_u32(&mut self) -> Result<u32, String> {
        let mut value = 0u32;
        for shift in (0..35).step_by(7) {
            let byte = *self.0.get(self.1).ok_or("a SWF label table is cut off")?;
            self.1 += 1;
            value |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                break;
            }
        }
        Ok(value)
    }

    fn string(&mut self) -> Result<String, String> {
        let rest = self.0.get(self.1..).ok_or("a SWF label is cut off")?;
        let end = rest.iter().position(|&b| b == 0).ok_or("a SWF label is cut off")?;
        self.1 += end + 1;
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::frame_labels;

    fn tag(code: u16, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        if body.len() < 0x3f {
            out.extend_from_slice(&(code << 6 | body.len() as u16).to_le_bytes());
        } else {
            out.extend_from_slice(&(code << 6 | 0x3f).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        }
        out.extend_from_slice(body);
        out
    }

    fn label(name: &str) -> Vec<u8> {
        [name.as_bytes(), &[0]].concat()
    }

    fn uncompressed(tags: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = [vec![0, 0, 24, 1, 0], tags.concat(), tag(0, &[])].concat();
        [b"FWS".as_slice(), &[10], &((body.len() + 8) as u32).to_le_bytes(), &body].concat()
    }

    #[test]
    fn main_timeline_frame_labels_are_the_cells_in_order() {
        let sprite = tag(39, &[[1, 0, 1, 0].as_slice(), &tag(43, &label("Inside")), &tag(0, &[])].concat());
        let swf = uncompressed(&[tag(43, &label("Enter")), tag(1, &[]), sprite, tag(43, &label("r2")), tag(1, &[]), tag(43, &label("Enter"))]);
        assert_eq!(frame_labels(&swf).unwrap(), ["Enter", "r2"]);
    }

    #[test]
    fn the_scene_label_table_wins_and_compressed_files_read_the_same() {
        let table = [&[1, 0][..], &label("Scene 1"), &[2, 0], &label("Wait"), &[1], &label("Enter")].concat();
        let plain = uncompressed(&[tag(86, &table), tag(43, &label("Stale"))]);
        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib.write_all(&plain[8..]).unwrap();
        let compressed = [b"CWS".as_slice(), &plain[3..8], &zlib.finish().unwrap()].concat();
        assert_eq!(frame_labels(&plain).unwrap(), ["Wait", "Enter"]);
        assert_eq!(frame_labels(&compressed).unwrap(), ["Wait", "Enter"]);
    }

    #[test]
    fn a_file_that_is_not_a_swf_is_an_error() {
        assert!(frame_labels(b"<html>not found</html>").is_err());
        assert!(frame_labels(b"FWS").is_err());
    }
}
