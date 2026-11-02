use std::borrow::Cow;
use std::fs::File;
use std::io::{self, LineWriter, Write};
use std::path::Path;
use std::time::Instant;

pub struct Recorder {
    out: LineWriter<File>,
    start: Instant,
}

impl Recorder {
    pub fn create(path: &Path) -> io::Result<Self> {
        Ok(Recorder { out: LineWriter::new(File::create(path)?), start: Instant::now() })
    }

    pub fn write(&mut self, dir: &str, msg: &str) -> io::Result<()> {
        let t = self.start.elapsed().as_millis();
        let msg = serde_json::to_string(&redact(msg))?;
        writeln!(self.out, r#"{{"t":{t},"dir":"{dir}","msg":{msg}}}"#)
    }
}

fn redact(msg: &str) -> Cow<'_, str> {
    [("<pword><![CDATA[", "]]>"), ("\"strEmail\":\"", "\""), ("\"strIP\":\"", "\"")]
        .into_iter()
        .fold(Cow::Borrowed(msg), |msg, (open, close)| {
            let Some(start) = msg.find(open).map(|i| i + open.len()) else { return msg };
            let Some(len) = msg[start..].find(close) else { return msg };
            let mut owned = msg.into_owned();
            owned.replace_range(start..start + len, "REDACTED");
            Cow::Owned(owned)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_the_token_email_and_ip() {
        assert_eq!(
            redact("<login z='zone_master'><nick><![CDATA[SPIDER#0001~hero~5.31]]></nick><pword><![CDATA[abc123]]></pword></login>"),
            "<login z='zone_master'><nick><![CDATA[SPIDER#0001~hero~5.31]]></nick><pword><![CDATA[REDACTED]]></pword></login>"
        );
        assert_eq!(
            redact(r#"{"cmd":"initUserData","data":{"strEmail":"hero@example.com","intLevel":100,"strIP":"123.456.789.012"}}"#),
            r#"{"cmd":"initUserData","data":{"strEmail":"REDACTED","intLevel":100,"strIP":"REDACTED"}}"#
        );
    }

    #[test]
    fn leaves_other_messages_alone() {
        assert!(matches!(redact("%xt%zm%moveToCell%7111%Enter%Spawn%"), Cow::Borrowed(_)));
    }
}
