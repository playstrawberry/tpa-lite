//! The contracts between the message scripts, over `ipc`. Both sides of each contract use this crate, so the
//! bytes are written and read in one place.
//!
//! UTF-8 text, fields separated by TAB. A player is his UUID as 32 hex digits (hyphens are accepted on reading).
//!
//! - **`ignore/1`**, answered by `ignore-lite` to any plugin: `ignore/1<TAB>check<TAB><reader><TAB><sender>`
//!   answers `ignore/1<TAB>1` when the reader ignores the sender and `ignore/1<TAB>0` otherwise (also for a
//!   reader who is not online). A script that carries words from one player to another (private messages, a
//!   teleport request, a chat format that sends its own lines) asks before it delivers. The answer is about the
//!   lists alone: a sender who holds [`IGNORE_EXEMPT_NODE`] is never ignored, and the asker, who has the
//!   player at hand, checks that first ([`runtime::ignores`] does).
//! - **`spies/1`**, answered by `socialspy-lite` to `msg-lite` only: `spies/1<TAB>list` answers `spies/1` and
//!   then one field per staff member who switched social spy on (at most [`MAX_SPIES`]). The asker shows a
//!   private message to each of them who still holds [`SPY_NODE`] at that moment.
//!
//! No answer (the other script is not installed) means "nobody ignores" and "nobody spies".
#![forbid(unsafe_code)]

#[cfg(target_arch = "wasm32")]
pub mod runtime;

/// A player: the two halves of his UUID.
pub type Id = (u64, u64);

/// The script that answers `ignore/1`.
pub const IGNORE_PLUGIN: &str = "ignore-lite";
/// The script that answers `spies/1`.
pub const SPY_PLUGIN: &str = "socialspy-lite";
/// The only script `spies/1` is answered to.
pub const MESSAGE_PLUGIN: &str = "msg-lite";
/// Who holds this cannot be ignored (staff): the asker of `ignore/1` checks it on the sender first.
pub const IGNORE_EXEMPT_NODE: &str = "ignore-lite:exempt";
/// The permission a social spy must hold when a message is shown to him.
pub const SPY_NODE: &str = "socialspy-lite:use";
/// The most social spies at once.
pub const MAX_SPIES: usize = 64;

/// The most online players one name suggestion looks at.
pub const MAX_SCAN: usize = 1024;
/// The most names one suggestion offers.
pub const MAX_SUGGESTIONS: usize = 20;

/// Whether `name` starts with what the player typed so far, without regard to ASCII case. Nothing typed
/// matches every name.
pub fn prefix_match(typed: &str, name: &str) -> bool {
    name.as_bytes()
        .get(..typed.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(typed.as_bytes()))
}

const IGNORE: &str = "ignore/1";
const SPIES: &str = "spies/1";

/// A player id as the contracts write it.
pub fn id_text(id: Id) -> String {
    format!("{:016x}{:016x}", id.0, id.1)
}

/// Reads a player id: 32 hex digits, with or without the UUID's hyphens.
pub fn parse_id(text: &str) -> Option<Id> {
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (high, low) = hex.split_at(16);
    Some((
        u64::from_str_radix(high, 16).ok()?,
        u64::from_str_radix(low, 16).ok()?,
    ))
}

fn fields(bytes: &[u8]) -> Option<Vec<&str>> {
    // Far above any message of these contracts; a longer one is not one of them.
    if bytes.len() > 4096 {
        return None;
    }
    Some(std::str::from_utf8(bytes).ok()?.split('\t').collect())
}

/// "Does `reader` ignore `sender`?"
pub fn ignore_request(reader: Id, sender: Id) -> Vec<u8> {
    format!("{IGNORE}\tcheck\t{}\t{}", id_text(reader), id_text(sender)).into_bytes()
}

/// The reader and the sender of an `ignore/1` question; `None` when the message is not one.
pub fn parse_ignore_request(bytes: &[u8]) -> Option<(Id, Id)> {
    match fields(bytes)?.as_slice() {
        [IGNORE, "check", reader, sender] => Some((parse_id(reader)?, parse_id(sender)?)),
        _ => None,
    }
}

pub fn ignore_answer(ignored: bool) -> Vec<u8> {
    format!("{IGNORE}\t{}", u8::from(ignored)).into_bytes()
}

/// `None` when the message is not an `ignore/1` answer.
pub fn parse_ignore_answer(bytes: &[u8]) -> Option<bool> {
    match fields(bytes)?.as_slice() {
        [IGNORE, "1"] => Some(true),
        [IGNORE, "0"] => Some(false),
        _ => None,
    }
}

/// "Who spies?"
pub fn spies_request() -> Vec<u8> {
    format!("{SPIES}\tlist").into_bytes()
}

pub fn is_spies_request(bytes: &[u8]) -> bool {
    fields(bytes).is_some_and(|f| f.as_slice() == [SPIES, "list"])
}

/// At most [`MAX_SPIES`] are written.
pub fn spies_answer(spies: &[Id]) -> Vec<u8> {
    let mut out = String::from(SPIES);
    for id in spies.iter().take(MAX_SPIES) {
        out.push('\t');
        out.push_str(&id_text(*id));
    }
    out.into_bytes()
}

/// `None` when the message is not a `spies/1` answer or names more than [`MAX_SPIES`].
pub fn parse_spies_answer(bytes: &[u8]) -> Option<Vec<Id>> {
    let fields = fields(bytes)?;
    let (head, spies) = fields.split_first()?;
    if *head != SPIES || spies.len() > MAX_SPIES {
        return None;
    }
    spies.iter().map(|text| parse_id(text)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Id = (0x0f1e_2d3c_4b5a_6978, 0x8796_a5b4_c3d2_e1f0);
    const B: Id = (0, 2);

    #[test]
    fn a_name_is_suggested_by_its_start_only() {
        assert!(prefix_match("", "Bob"));
        assert!(prefix_match("bo", "Bob"));
        assert!(prefix_match("BOB", "Bob"));
        assert!(!prefix_match("ob", "Bob"));
        assert!(!prefix_match("Bobby", "Bob"));
        assert!(!prefix_match("ç", "Bob"));
    }

    #[test]
    fn a_player_id_reads_back_with_and_without_hyphens() {
        assert_eq!(id_text(A), "0f1e2d3c4b5a69788796a5b4c3d2e1f0");
        assert_eq!(parse_id(&id_text(A)), Some(A));
        assert_eq!(parse_id("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"), Some(A));
        assert_eq!(parse_id("00000000-0000-0000-0000-000000000002"), Some(B));
        for bad in [
            "",
            "0f1e",
            "zf1e2d3c4b5a69788796a5b4c3d2e1f0",
            "0f1e2d3c4b5a69788796a5b4c3d2e1f00",
        ] {
            assert_eq!(parse_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn ignore_messages_are_the_contract_bytes() {
        let request = ignore_request(A, B);
        assert_eq!(
            request,
            b"ignore/1\tcheck\t0f1e2d3c4b5a69788796a5b4c3d2e1f0\t00000000000000000000000000000002"
        );
        assert_eq!(parse_ignore_request(&request), Some((A, B)));
        assert_eq!(parse_ignore_answer(&ignore_answer(true)), Some(true));
        assert_eq!(parse_ignore_answer(&ignore_answer(false)), Some(false));
        for bad in [
            &b"ignore/2\tcheck\ta\tb"[..],
            b"ignore/1\tcheck\tx\ty",
            b"ignore/1\tcheck",
            b"\xff",
            b"",
        ] {
            assert_eq!(parse_ignore_request(bad), None);
        }
        for bad in [&b"ignore/1\t2"[..], b"ignore/1", b"spies/1\t1", b"1"] {
            assert_eq!(parse_ignore_answer(bad), None);
        }
        assert_eq!(parse_ignore_request(&vec![b'a'; 5000]), None);
    }

    #[test]
    fn spy_messages_are_the_contract_bytes_and_bounded() {
        assert_eq!(spies_request(), b"spies/1\tlist");
        assert!(is_spies_request(&spies_request()));
        assert!(!is_spies_request(b"spies/1"));
        assert!(!is_spies_request(&ignore_request(A, B)));
        assert_eq!(parse_spies_answer(&spies_answer(&[])), Some(vec![]));
        assert_eq!(parse_spies_answer(&spies_answer(&[A, B])), Some(vec![A, B]));
        let many: Vec<Id> = (0..100).map(|n| (n, n)).collect();
        assert_eq!(
            parse_spies_answer(&spies_answer(&many)).map(|s| s.len()),
            Some(MAX_SPIES)
        );
        let mut long = String::from("spies/1");
        for _ in 0..=MAX_SPIES {
            long.push('\t');
            long.push_str(&id_text(A));
        }
        assert_eq!(parse_spies_answer(long.as_bytes()), None);
        assert_eq!(parse_spies_answer(b"spies/1\tnot-an-id"), None);
        assert_eq!(parse_spies_answer(b"ignore/1\t1"), None);
    }
}
