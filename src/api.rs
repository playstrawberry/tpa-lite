//! The service for other plugins (a settings menu, for example), over plugin messages.
//!
//! Requests (UTF-8 text):
//! - `requests-enabled <uuid>`
//! - `set-requests-enabled <uuid> <true|false>`
//!
//! The answer is `ok true` or `ok false`: the player's choice after the call. A plugin has to be
//! named in `preference_api_plugins`. The player does not have to be online. Sending the same
//! `set` twice changes nothing the second time.
use crate::prefs::{self, Preferences};

const MAX_MESSAGE: usize = 128;

#[derive(Debug, PartialEq, Eq)]
pub enum Call {
    Get(u128),
    Set(u128, bool),
}

pub fn parse(message: &[u8]) -> Result<Call, String> {
    const UNKNOWN: &str =
        "write `requests-enabled <uuid>` or `set-requests-enabled <uuid> <true|false>`";
    if message.len() > MAX_MESSAGE {
        return Err(UNKNOWN.to_owned());
    }
    let text = std::str::from_utf8(message).map_err(|_| UNKNOWN.to_owned())?;
    let uuid = |id: &str| prefs::parse_uuid(id).ok_or_else(|| UNKNOWN.to_owned());
    match text.split(' ').collect::<Vec<_>>().as_slice() {
        ["requests-enabled", id] => Ok(Call::Get(uuid(id)?)),
        ["set-requests-enabled", id, "true"] => Ok(Call::Set(uuid(id)?, true)),
        ["set-requests-enabled", id, "false"] => Ok(Call::Set(uuid(id)?, false)),
        _ => Err(UNKNOWN.to_owned()),
    }
}

/// Answers a message from `from`. The bool says the file has to be written.
pub fn handle(
    preferences: &mut Preferences,
    allowed: &[String],
    from: &str,
    message: &[u8],
) -> Result<(Vec<u8>, bool), String> {
    if !allowed.iter().any(|plugin| plugin == from) {
        return Err("this plugin is not in preference_api_plugins".to_owned());
    }
    let (id, changed) = match parse(message)? {
        Call::Get(id) => (id, false),
        Call::Set(id, enabled) => (
            id,
            preferences
                .set(id, enabled)
                .map_err(|_| "the preference list is full".to_owned())?,
        ),
    };
    let answer = format!("ok {}", preferences.enabled(id));
    Ok((answer.into_bytes(), changed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "00000000-0000-0000-0000-000000000007";

    fn allowed() -> Vec<String> {
        vec!["menu-plugin".to_owned()]
    }
    fn ask(p: &mut Preferences, from: &str, text: &str) -> Result<(Vec<u8>, bool), String> {
        handle(p, &allowed(), from, text.as_bytes())
    }

    #[test]
    fn requests_are_read_strictly() {
        assert_eq!(
            parse(format!("requests-enabled {ID}").as_bytes()),
            Ok(Call::Get(7))
        );
        assert_eq!(
            parse(format!("set-requests-enabled {ID} false").as_bytes()),
            Ok(Call::Set(7, false))
        );
        for bad in [
            String::new(),
            "requests-enabled".to_owned(),
            format!("requests-enabled {ID} true"),
            format!("set-requests-enabled {ID}"),
            format!("set-requests-enabled {ID} maybe"),
            format!("set-requests-enabled {ID} false extra"),
            "requests-enabled not-a-uuid".to_owned(),
            format!("something-else {ID}"),
            "x".repeat(MAX_MESSAGE + 1),
        ] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(parse(&[0xff, 0xfe]).is_err());
    }
    #[test]
    fn a_plugin_not_on_the_list_gets_nothing_and_changes_nothing() {
        let mut p = Preferences::default();
        let set = format!("set-requests-enabled {ID} false");
        assert!(ask(&mut p, "other", &set).is_err());
        assert!(handle(&mut p, &[], "menu-plugin", set.as_bytes()).is_err());
        assert!(p.enabled(7));
        assert!(ask(&mut p, "other", &format!("requests-enabled {ID}")).is_err());
    }
    #[test]
    fn get_and_set_work_for_an_offline_uuid_and_a_repeat_changes_nothing() {
        let mut p = Preferences::default();
        let get = format!("requests-enabled {ID}");
        let set = format!("set-requests-enabled {ID} false");
        assert_eq!(
            ask(&mut p, "menu-plugin", &get),
            Ok((b"ok true".to_vec(), false))
        );
        assert_eq!(
            ask(&mut p, "menu-plugin", &set),
            Ok((b"ok false".to_vec(), true))
        );
        assert_eq!(
            ask(&mut p, "menu-plugin", &set),
            Ok((b"ok false".to_vec(), false))
        );
        assert_eq!(
            ask(&mut p, "menu-plugin", &get),
            Ok((b"ok false".to_vec(), false))
        );
    }
    #[test]
    fn a_full_set_is_an_error_and_keeps_the_old_answer() {
        let mut p = Preferences::default();
        for i in 100..(100 + prefs::MAX_ENTRIES as u128) {
            p.set(i, false).unwrap();
        }
        assert!(
            ask(
                &mut p,
                "menu-plugin",
                &format!("set-requests-enabled {ID} false")
            )
            .is_err()
        );
        assert!(p.enabled(7));
    }
}
