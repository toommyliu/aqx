use crate::MAX_MESSAGE_BYTES;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("message longer than {MAX_MESSAGE_BYTES} bytes")]
pub struct FrameTooLong;

#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
    start: usize,
}

impl Framer {
    pub fn push(&mut self, data: &[u8]) -> Result<(), FrameTooLong> {
        self.buf.drain(..self.start);
        self.start = 0;
        self.buf.extend_from_slice(data);
        let partial = self.buf.len()
            - self
                .buf
                .iter()
                .rposition(|&b| b == 0)
                .map_or(0, |end| end + 1);
        if partial > MAX_MESSAGE_BYTES {
            self.buf.clear();
            return Err(FrameTooLong);
        }
        Ok(())
    }

    pub fn next_frame(&mut self) -> Option<&[u8]> {
        loop {
            let len = self.buf[self.start..].iter().position(|&b| b == 0)?;
            let frame = self.start..self.start + len;
            self.start += len + 1;
            if !frame.is_empty() {
                return Some(&self.buf[frame]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(framer: &mut Framer, data: &[u8]) -> Vec<String> {
        framer.push(data).unwrap();
        let mut out = Vec::new();
        while let Some(frame) = framer.next_frame() {
            out.push(String::from_utf8(frame.to_vec()).unwrap());
        }
        out
    }

    #[test]
    fn splits_on_nul_and_keeps_the_partial_tail() {
        let mut framer = Framer::default();
        assert_eq!(frames(&mut framer, b"%xt%hi%-1%\0%xt%serv"), ["%xt%hi%-1%"]);
        assert_eq!(frames(&mut framer, b"er%-1%ok%\0\0"), ["%xt%server%-1%ok%"]);
        assert!(frames(&mut framer, b"").is_empty());
    }

    #[test]
    fn rejects_a_partial_frame_past_the_limit() {
        let mut framer = Framer::default();
        let chunk = vec![b'a'; MAX_MESSAGE_BYTES / 2 + 1];
        framer.push(&chunk).unwrap();
        assert_eq!(framer.push(&chunk), Err(FrameTooLong));
    }
}
