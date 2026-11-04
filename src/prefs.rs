//! Who refuses teleport requests: one persistent set, the only authority for commands and for
//! other plugins. Most players keep the default, so only the ones who turned requests off are
//! stored, and a lookup is one set read with no disk access.
use std::collections::BTreeSet;

pub const FILE: &str = "requests-off.txt";
pub const MAX_ENTRIES: usize = 65_536;
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const VERSION: &str = "version=1";

#[derive(Default, Debug)]
pub struct Preferences {
    off: BTreeSet<u128>,
    dirty: bool,
}

/// Reads a UUID written the usual way, 8-4-4-4-12 hexadecimal digits.
pub fn parse_uuid(text: &str) -> Option<u128> {
    let bytes = text.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let mut value = 0u128;
    for (i, b) in bytes.iter().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if *b != b'-' {
                return None;
            }
        } else {
            value = (value << 4) | u128::from((*b as char).to_digit(16)?);
        }
    }
    Some(value)
}

fn format_uuid(id: u128) -> String {
    let hex = format!("{id:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

impl Preferences {
    /// Reads the file's text. A file this version cannot fully read is refused, never trimmed.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        const BAD: &str = "preferences_error";
        let mut lines = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'));
        let mut off = BTreeSet::new();
        match lines.next() {
            None => return Ok(Self::default()),
            Some(VERSION) => {}
            Some(_) => return Err(BAD),
        }
        for line in lines {
            off.insert(parse_uuid(line).ok_or(BAD)?);
            if off.len() > MAX_ENTRIES {
                return Err(BAD);
            }
        }
        Ok(Self { off, dirty: false })
    }
    pub fn render(&self) -> String {
        let mut text = String::with_capacity(VERSION.len() + 1 + self.off.len() * 37);
        text.push_str(VERSION);
        text.push('\n');
        for id in &self.off {
            text.push_str(&format_uuid(*id));
            text.push('\n');
        }
        text
    }
    pub fn enabled(&self, id: u128) -> bool {
        !self.off.contains(&id)
    }
    /// Sets the choice and returns whether anything changed.
    pub fn set(&mut self, id: u128, enabled: bool) -> Result<bool, &'static str> {
        let changed = if enabled {
            self.off.remove(&id)
        } else {
            if !self.off.contains(&id) && self.off.len() >= MAX_ENTRIES {
                return Err("full");
            }
            self.off.insert(id)
        };
        self.dirty |= changed;
        Ok(changed)
    }
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = "00000000-0000-0000-0000-000000000001";
    const TWO: &str = "8667ba71-b85a-4004-af54-457a9734eed7";

    #[test]
    fn uuids_are_read_strictly() {
        assert_eq!(parse_uuid(ONE), Some(1));
        assert_eq!(
            parse_uuid("8667BA71-B85A-4004-AF54-457A9734EED7"),
            parse_uuid(TWO)
        );
        for bad in [
            "",
            "00000000000000000000000000000001",
            "00000000-0000-0000-0000-00000000000",
            "00000000-0000-0000-0000-0000000000011",
            "0000000-00000-0000-0000-000000000001",
            "z0000000-0000-0000-0000-000000000001",
            "+0000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-00000000000\u{e9}",
        ] {
            assert_eq!(parse_uuid(bad), None, "{bad}");
        }
    }
    #[test]
    fn requests_are_on_until_turned_off() {
        let mut p = Preferences::default();
        assert!(p.enabled(1));
        assert_eq!(p.set(1, true), Ok(false));
        assert!(!p.take_dirty());
        assert_eq!(p.set(1, false), Ok(true));
        assert!(!p.enabled(1));
        assert!(p.take_dirty());
        assert!(!p.take_dirty());
        assert_eq!(p.set(1, false), Ok(false));
        assert!(!p.take_dirty());
        assert_eq!(p.set(1, true), Ok(true));
        assert!(p.enabled(1));
    }
    #[test]
    fn the_file_round_trips_and_keeps_offline_players() {
        let mut p = Preferences::default();
        p.set(parse_uuid(TWO).unwrap(), false).unwrap();
        p.set(1, false).unwrap();
        let text = p.render();
        assert!(text.contains(TWO) && text.contains(ONE));
        let again = Preferences::parse(&text).unwrap();
        assert!(!again.enabled(1));
        assert!(!again.enabled(parse_uuid(TWO).unwrap()));
        assert!(again.enabled(2));
        assert_eq!(again.render(), text);
    }
    #[test]
    fn an_imported_file_may_have_comments_blanks_and_repeats() {
        let text = format!("# an import\nversion=1\n\n{ONE}\n# more\n{ONE}\n  {TWO}  \n");
        let p = Preferences::parse(&text).unwrap();
        assert!(!p.enabled(1));
        assert!(!p.enabled(parse_uuid(TWO).unwrap()));
        assert!(!p.take_dirty_view());
    }
    #[test]
    fn an_empty_file_is_empty_but_a_bad_one_is_refused() {
        assert!(Preferences::parse("").unwrap().enabled(1));
        for bad in [
            ONE.to_owned(),
            format!("version=2\n{ONE}\n"),
            format!("version=1\nversion=1\n{ONE}\n"),
            format!("version=1\n{ONE}\nnot-a-uuid\n"),
        ] {
            assert!(Preferences::parse(&bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn the_set_is_bounded_on_a_change_and_on_import() {
        let mut p = Preferences::default();
        for i in 0..MAX_ENTRIES as u128 {
            p.set(i, false).unwrap();
        }
        assert!(p.set(MAX_ENTRIES as u128, false).is_err());
        assert!(p.enabled(MAX_ENTRIES as u128));
        assert_eq!(p.set(0, true), Ok(true));
        assert!(p.set(MAX_ENTRIES as u128, false).is_ok());
        let mut text = String::from("version=1\n");
        for i in 0..=MAX_ENTRIES {
            text.push_str(&format!("00000000-0000-0000-0000-{i:012x}\n"));
        }
        assert!(Preferences::parse(&text).is_err());
    }

    impl Preferences {
        fn take_dirty_view(&self) -> bool {
            self.dirty
        }
    }
}
