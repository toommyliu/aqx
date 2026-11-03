use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Default)]
pub struct Mask {
    secrets: Vec<(String, String)>,
}

impl Mask {
    pub fn new(home: Option<String>, accounts: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut secrets: Vec<(String, String)> = home.map(|home| (home, "~".to_string())).into_iter().chain(accounts).filter(|(secret, _)| !secret.is_empty()).collect();
        secrets.sort_by_key(|(secret, _)| std::cmp::Reverse(secret.len()));
        Mask { secrets }
    }

    pub fn hide(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        while let Some(c) = text[at..].chars().next() {
            match self.secrets.iter().find(|(secret, _)| found(text, at, secret)) {
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

    pub fn hide_in<T: Serialize + DeserializeOwned>(&self, value: &T) -> Option<T> {
        let mut json = serde_json::to_value(value).ok()?;
        self.hide_value(&mut json);
        serde_json::from_value(json).ok()
    }

    fn hide_value(&self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.hide(text),
            Value::Array(items) => items.iter_mut().for_each(|item| self.hide_value(item)),
            Value::Object(fields) => fields.values_mut().for_each(|field| self.hide_value(field)),
            _ => {}
        }
    }
}

fn found(text: &str, at: usize, secret: &str) -> bool {
    let Some(head) = text.get(at..at + secret.len()) else { return false };
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
        Mask::new(Some("/Users/sam".into()), [("Hero".into(), "account 1".into()), ("hero alt".into(), "account 2".into())])
    }

    #[test]
    fn names_become_aliases_in_any_case_and_the_home_becomes_a_tilde() {
        assert_eq!(mask().hide("logging in as HERO"), "logging in as account 1");
        assert_eq!(mask().hide("running /Users/sam/bots/farm.ts"), "running ~/bots/farm.ts");
        assert_eq!(mask().hide("hero alt joined"), "account 2 joined");
    }

    #[test]
    fn a_name_inside_a_longer_word_stays() {
        assert_eq!(mask().hide("Heroic superhero hero_x hero."), "Heroic superhero hero_x account 1.");
    }

    #[test]
    fn nothing_changes_without_secrets() {
        assert_eq!(Mask::default().hide("hero ✓ /Users/sam"), "hero ✓ /Users/sam");
    }
}
