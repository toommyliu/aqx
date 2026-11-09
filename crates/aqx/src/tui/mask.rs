#[derive(Default)]
pub struct Mask {
    secrets: Vec<(String, String)>,
}

impl Mask {
    pub fn new(home: Option<String>, names: impl IntoIterator<Item = (String, String)>) -> Mask {
        let mut secrets: Vec<(String, String)> = home
            .map(|home| (home, "~".to_owned()))
            .into_iter()
            .chain(names)
            .filter(|(secret, _)| !secret.is_empty())
            .collect();
        secrets.sort_by_key(|(secret, _)| std::cmp::Reverse(secret.len()));
        Mask { secrets }
    }

    pub fn hide(&self, text: &str) -> String {
        if self.secrets.is_empty() {
            return text.to_owned();
        }
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        while let Some(c) = text[at..].chars().next() {
            match self
                .secrets
                .iter()
                .find(|(secret, _)| found(text, at, secret))
            {
                Some((secret, alias)) => {
                    out.push_str(alias);
                    at += secret.len();
                }
                None => {
                    out.push(c);
                    at += c.len_utf8();
                }
            }
        }
        out
    }
}

fn found(text: &str, at: usize, secret: &str) -> bool {
    let Some(head) = text.get(at..at + secret.len()) else {
        return false;
    };
    head.eq_ignore_ascii_case(secret)
        && !(secret.starts_with(is_word) && text[..at].ends_with(is_word))
        && !(secret.ends_with(is_word) && text[at + secret.len()..].starts_with(is_word))
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask() -> Mask {
        Mask::new(
            Some("/Users/player1".into()),
            [
                ("player1".into(), "account 1".into()),
                ("player1 alt".into(), "account 2".into()),
            ],
        )
    }

    #[test]
    fn names_become_aliases_in_any_case_and_the_home_becomes_a_tilde() {
        assert_eq!(
            mask().hide("logging in as PLAYER1"),
            "logging in as account 1"
        );
        assert_eq!(
            mask().hide("running /Users/player1/farm.ts"),
            "running ~/farm.ts"
        );
        assert_eq!(
            mask().hide("player1 alt joined"),
            "account 2 joined",
            "the longer name wins"
        );
    }

    #[test]
    fn a_name_inside_a_longer_word_stays() {
        assert_eq!(
            mask().hide("player12 xplayer1 player1_x player1."),
            "player12 xplayer1 player1_x account 1."
        );
    }

    #[test]
    fn nothing_changes_without_secrets() {
        assert_eq!(
            Mask::default().hide("player1 ✓ /Users/player1"),
            "player1 ✓ /Users/player1"
        );
    }
}
