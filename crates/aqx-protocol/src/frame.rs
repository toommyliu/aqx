use std::io::Read;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::read::{DeflateDecoder, ZlibDecoder};

pub const MAX_FRAME: usize = 16 << 20;

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    TooLong,
    Base64,
    Inflate,
    Utf8,
}

#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
}

impl Framer {
    pub fn feed(&mut self, data: &[u8], mut on_frame: impl FnMut(&[u8])) -> Result<(), FrameError> {
        self.buf.extend_from_slice(data);
        let mut start = 0;
        for end in memchr::memchr_iter(0, &self.buf) {
            if end > start {
                on_frame(&self.buf[start..end]);
            }
            start = end + 1;
        }
        self.buf.drain(..start);
        if self.buf.len() > MAX_FRAME {
            self.buf.clear();
            return Err(FrameError::TooLong);
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct Decoder {
    packed: Vec<u8>,
    text: String,
}

impl Decoder {
    pub fn decode<'a>(&'a mut self, frame: &'a [u8]) -> Result<&'a str, FrameError> {
        let Some(encoded) = frame.strip_prefix(b"Z") else {
            return std::str::from_utf8(frame).map_err(|_| FrameError::Utf8);
        };
        self.packed.clear();
        STANDARD.decode_vec(encoded, &mut self.packed).map_err(|_| FrameError::Base64)?;
        self.text.clear();
        let limit = MAX_FRAME as u64 + 1;
        if ZlibDecoder::new(&self.packed[..]).take(limit).read_to_string(&mut self.text).is_err() {
            self.text.clear();
            DeflateDecoder::new(&self.packed[..])
                .take(limit)
                .read_to_string(&mut self.text)
                .map_err(|_| FrameError::Inflate)?;
        }
        if self.text.len() > MAX_FRAME {
            return Err(FrameError::TooLong);
        }
        Ok(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::{DeflateEncoder, ZlibEncoder};

    use super::*;

    fn frames(framer: &mut Framer, data: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        framer.feed(data, |f| out.push(String::from_utf8(f.to_vec()).unwrap())).unwrap();
        out
    }

    #[test]
    fn splits_on_nul_and_keeps_the_partial_tail() {
        let mut framer = Framer::default();
        assert_eq!(frames(&mut framer, b"%xt%hi%-1%\0%xt%serv"), ["%xt%hi%-1%"]);
        assert_eq!(frames(&mut framer, b"er%-1%You joined%\0\0"), ["%xt%server%-1%You joined%"]);
        assert_eq!(frames(&mut framer, b""), Vec::<String>::new());
    }

    #[test]
    fn rejects_a_frame_longer_than_the_limit() {
        let mut framer = Framer::default();
        let chunk = vec![b'a'; MAX_FRAME / 2 + 1];
        framer.feed(&chunk, |_| {}).unwrap();
        assert_eq!(framer.feed(&chunk, |_| {}), Err(FrameError::TooLong));
    }

    fn z(packed: Vec<u8>) -> Vec<u8> {
        [b"Z".as_slice(), STANDARD.encode(packed).as_bytes()].concat()
    }

    const MSG: &str = r#"{"t":"xt","b":{"r":-1,"o":{"cmd":"initUserData","uid":1002}}}"#;

    #[test]
    fn passes_plain_text_through() {
        assert_eq!(Decoder::default().decode(MSG.as_bytes()), Ok(MSG));
    }

    #[test]
    fn inflates_zlib_and_raw_deflate() {
        let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib.write_all(MSG.as_bytes()).unwrap();
        let mut raw = DeflateEncoder::new(Vec::new(), Compression::default());
        raw.write_all(MSG.as_bytes()).unwrap();
        let mut decoder = Decoder::default();
        assert_eq!(decoder.decode(&z(zlib.finish().unwrap())), Ok(MSG));
        assert_eq!(decoder.decode(&z(raw.finish().unwrap())), Ok(MSG));
    }

    #[test]
    fn reports_corrupt_frames() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.decode(b"Z!!!"), Err(FrameError::Base64));
        assert_eq!(decoder.decode(&z(b"not deflate".to_vec())), Err(FrameError::Inflate));
        assert_eq!(decoder.decode(b"\xff\xfe"), Err(FrameError::Utf8));
    }
}
