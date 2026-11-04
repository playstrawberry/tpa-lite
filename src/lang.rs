use std::{collections::BTreeMap, io::Read, path::Path};

const FILES: [(&str, &str); 4] = [
    ("en", include_str!("../lang/en.lang")),
    ("fr", include_str!("../lang/fr.lang")),
    ("es", include_str!("../lang/es.lang")),
    ("tr", include_str!("../lang/tr.lang")),
];
const MAX_OVERRIDE_BYTES: u64 = 64 * 1024;
const MAX_OVERRIDE_VALUE: usize = 512;

pub fn language(locale: &str) -> &'static str {
    let code = locale.get(..2).unwrap_or("").to_ascii_lowercase();
    FILES
        .iter()
        .map(|(c, _)| *c)
        .find(|c| *c == code)
        .unwrap_or("en")
}
fn lookup<'a>(file: &'a str, key: &str) -> Option<&'a str> {
    file.lines()
        .filter_map(|s| s.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}
pub fn fill(raw: &str, args: &[&str]) -> String {
    let mut out = raw.to_owned();
    for (i, arg) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), arg);
    }
    out
}
pub fn text(code: &str, key: &str, args: &[&str]) -> String {
    let raw = FILES
        .iter()
        .find(|(c, _)| *c == code)
        .and_then(|(_, file)| lookup(file, key))
        .or_else(|| lookup(FILES[0].1, key))
        .unwrap_or(key);
    fill(raw, args)
}

/// A server owner's own texts: `lang/<code>.lang` in the data folder, read at load only.
#[derive(Default, Debug)]
pub struct Overrides(BTreeMap<String, BTreeMap<String, String>>);

impl Overrides {
    pub fn load(folder: &Path) -> Result<Self, &'static str> {
        let mut all = Self::default();
        for (code, _) in FILES {
            let path = folder.join(format!("{code}.lang"));
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err("messages_error"),
            };
            let mut text = String::new();
            file.take(MAX_OVERRIDE_BYTES + 1)
                .read_to_string(&mut text)
                .map_err(|_| "messages_error")?;
            all.add(code, &text)?;
        }
        Ok(all)
    }
    /// Checks one file's text: known keys, no repeats, no placeholder English does not have.
    pub fn add(&mut self, code: &str, text: &str) -> Result<(), &'static str> {
        const BAD: &str = "messages_error";
        if text.len() as u64 > MAX_OVERRIDE_BYTES {
            return Err(BAD);
        }
        let mut file = BTreeMap::new();
        for line in text
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        {
            let (key, value) = line.split_once('=').ok_or(BAD)?;
            let english = lookup(FILES[0].1, key).ok_or(BAD)?;
            if value.len() > MAX_OVERRIDE_VALUE || !placeholders_known(value, english) {
                return Err(BAD);
            }
            if file.insert(key.to_owned(), expand_hex(value)).is_some() {
                return Err(BAD);
            }
        }
        self.0.insert(code.to_owned(), file);
        Ok(())
    }
    /// The owner's text for this language, if he wrote one. Empty means "send nothing".
    pub fn get(&self, code: &str, key: &str) -> Option<&str> {
        self.0.get(code)?.get(key).map(String::as_str)
    }
}

/// Writes `&#rrggbb` the way the legacy text parser reads hex colours.
pub fn expand_hex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("&#") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        match after
            .get(..6)
            .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            Some(hex) => {
                out.push_str("&x");
                for digit in hex.chars() {
                    out.push('&');
                    out.push(digit);
                }
                rest = &after[6..];
            }
            None => {
                out.push_str("&#");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether every `{n}` in `value` is one the English text fills in.
fn placeholders_known(value: &str, english: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.windows(3).enumerate().all(|(i, w)| {
        !(w[0] == b'{' && w[1].is_ascii_digit() && w[2] == b'}')
            || english.contains(&value[i..i + 3])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn languages_match_keys_and_all_placeholder_occurrences() {
        let entries = |s: &str| {
            s.lines()
                .filter_map(|s| s.split_once('='))
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect::<BTreeMap<_, _>>()
        };
        let en = entries(FILES[0].1);
        assert!(en.len() >= 30);
        for (code, file) in FILES {
            let translated = entries(file);
            assert_eq!(
                translated.len(),
                file.lines().count(),
                "duplicate key: {code}"
            );
            assert_eq!(
                en.keys().collect::<Vec<_>>(),
                translated.keys().collect::<Vec<_>>()
            );
            for (k, value) in &en {
                for slot in ["{0}", "{1}", "{2}"] {
                    assert_eq!(
                        value.matches(slot).count(),
                        translated[k].matches(slot).count(),
                        "{code} {k} {slot}"
                    );
                }
            }
        }
    }
    #[test]
    fn locale_and_english_fallback() {
        assert_eq!(language("FR_fr"), "fr");
        assert_eq!(language("tr_tr"), "tr");
        assert_eq!(language("es_es"), "es");
        assert_eq!(language("\u{0130}"), "en");
        assert_eq!(language("de_de"), "en");
        assert_eq!(
            text("unknown", "sent", &["Alex"]),
            text("en", "sent", &["Alex"])
        );
        assert_eq!(text("en", "unknown_key", &[]), "unknown_key");
    }
    #[test]
    fn an_override_is_per_language_and_blank_means_nothing_is_sent() {
        let mut o = Overrides::default();
        o.add("tr", "# yorum\n\nsent=&d{0} oyuncusuna gitti\nexpired=\n")
            .unwrap();
        assert_eq!(o.get("tr", "sent"), Some("&d{0} oyuncusuna gitti"));
        assert_eq!(o.get("tr", "expired"), Some(""));
        assert_eq!(o.get("tr", "denied"), None);
        assert_eq!(o.get("en", "sent"), None);
    }
    #[test]
    fn an_override_may_leave_out_a_placeholder() {
        let mut o = Overrides::default();
        o.add("en", "sent=Sent.").unwrap();
    }
    #[test]
    fn hex_colours_in_an_override_are_expanded_at_load() {
        let mut o = Overrides::default();
        o.add("en", "denied=&#9B59b6No").unwrap();
        assert_eq!(o.get("en", "denied"), Some("&x&9&B&5&9&b&6No"));
    }
    #[test]
    fn hex_expansion_leaves_everything_else_alone() {
        assert_eq!(
            expand_hex("&#123456a&#abcdefb"),
            "&x&1&2&3&4&5&6a&x&a&b&c&d&e&fb"
        );
        for same in [
            "&#12345",
            "&#12345g",
            "&a #123456",
            "plain",
            "&",
            "&#",
            "\u{e7}&#1",
        ] {
            assert_eq!(expand_hex(same), same);
        }
    }
    #[test]
    fn a_bad_override_file_is_refused() {
        for bad in [
            "no_such_key=x".to_owned(),
            "garbage".to_owned(),
            "sent=a\nsent=b".to_owned(),
            "sent={1}".to_owned(),
            "sent={0}{7}".to_owned(),
            format!("sent={}", "x".repeat(MAX_OVERRIDE_VALUE + 1)),
            "x".repeat(MAX_OVERRIDE_BYTES as usize + 1),
        ] {
            assert!(Overrides::default().add("en", &bad).is_err(), "{bad:.40}");
        }
    }
}
