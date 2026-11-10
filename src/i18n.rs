use std::collections::BTreeMap;
use std::env;
use std::sync::Arc;

const EN_MESSAGES: &str = include_str!("../locales/en.json");
const ZH_CN_MESSAGES: &str = include_str!("../locales/zh_CN.json");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    English,
    SimplifiedChinese,
}

#[derive(Clone)]
pub struct Translator {
    messages: Arc<BTreeMap<String, String>>,
    fallback: Arc<BTreeMap<String, String>>,
    language: Language,
}

impl Translator {
    pub fn system() -> Self {
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|name| env::var(name).ok().filter(|value| !value.is_empty()))
            .unwrap_or_else(|| "en".to_owned());
        Self::for_locale(&locale)
    }

    pub fn for_locale(locale: &str) -> Self {
        let fallback = parse_messages(EN_MESSAGES);
        let language = if locale.to_ascii_lowercase().starts_with("zh") {
            Language::SimplifiedChinese
        } else {
            Language::English
        };
        let messages = match language {
            Language::English => Arc::clone(&fallback),
            Language::SimplifiedChinese => parse_messages(ZH_CN_MESSAGES),
        };
        Self {
            messages,
            fallback,
            language,
        }
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn text(&self, key: &str) -> String {
        self.messages
            .get(key)
            .or_else(|| self.fallback.get(key))
            .cloned()
            .unwrap_or_else(|| key.to_owned())
    }

    pub fn format(&self, key: &str, values: &[(&str, &str)]) -> String {
        let mut text = self.text(key);
        for (name, value) in values {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }
}

fn parse_messages(source: &str) -> Arc<BTreeMap<String, String>> {
    Arc::new(
        serde_json::from_str(source)
            .expect("embedded translation catalog must contain a valid string map"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_simplified_chinese_for_zh_locales() {
        let translator = Translator::for_locale("zh_CN.UTF-8");
        assert_eq!(translator.language(), Language::SimplifiedChinese);
        assert_eq!(translator.text("menu.empty"), "暂无记录");
    }

    #[test]
    fn falls_back_to_english() {
        let translator = Translator::for_locale("fr_FR.UTF-8");
        assert_eq!(translator.language(), Language::English);
        assert_eq!(translator.text("menu.empty"), "No History");
    }

    #[test]
    fn unknown_keys_are_returned_unchanged() {
        let translator = Translator::for_locale("en");
        assert_eq!(translator.text("missing.key"), "missing.key");
    }

    #[test]
    fn english_and_chinese_catalogs_have_the_same_keys() {
        let english = parse_messages(EN_MESSAGES);
        let chinese = parse_messages(ZH_CN_MESSAGES);
        assert_eq!(
            english.keys().collect::<Vec<_>>(),
            chinese.keys().collect::<Vec<_>>()
        );
    }
}
