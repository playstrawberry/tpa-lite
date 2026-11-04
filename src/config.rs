use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
};

const BAD: &str = "config_error";
const MAX_ALIASES: usize = 8;
const MODES: [&str; 4] = ["survival", "creative", "adventure", "spectator"];
/// The platform's commands: a plugin never takes these names.
const RESERVED: [&str; 1] = ["marketplace"];

/// Every command, by the name its aliases are configured under.
pub const COMMANDS: [&str; 6] = [
    "tpa",
    "tpahere",
    "tpaccept",
    "tpdeny",
    "tpacancel",
    "tptoggle",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// 0 means a request does not expire by itself.
    pub expiry_ms: u64,
    pub delay_ms: u64,
    pub cooldown_ms: u64,
    pub max_pending: usize,
    pub partial_names: bool,
    pub buttons: bool,
    pub toggle_subcommand: bool,
    pub safe_teleport: bool,
    pub safe_search_radius: i32,
    pub water_safe: bool,
    /// Game modes that skip the safety check, as written in the file.
    pub safe_skip_modes: Vec<String>,
    pub center_on_block: bool,
    pub dismount: bool,
    pub arrival_protection_ms: u64,
    pub aliases: BTreeMap<String, Vec<String>>,
    pub preference_api_plugins: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            expiry_ms: 60_000,
            delay_ms: 3_000,
            cooldown_ms: 30_000,
            max_pending: 5,
            partial_names: false,
            buttons: true,
            toggle_subcommand: true,
            safe_teleport: true,
            safe_search_radius: 3,
            water_safe: false,
            safe_skip_modes: vec!["creative".into(), "spectator".into()],
            center_on_block: true,
            dismount: true,
            arrival_protection_ms: 0,
            aliases: BTreeMap::new(),
            preference_api_plugins: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, &'static str> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(_) => return Err("config_error"),
        };
        let mut text = String::new();
        file.take(4097)
            .read_to_string(&mut text)
            .map_err(|_| "config_error")?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, &'static str> {
        if text.len() > 4096 {
            return Err(BAD);
        }
        let mut cfg = Self::default();
        let mut seen = BTreeSet::new();
        for line in text
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
        {
            let (key, value) = line.split_once('=').ok_or(BAD)?;
            let (key, value) = (key.trim(), value.trim());
            if !seen.insert(key) {
                return Err(BAD);
            }
            match key {
                "expiry_seconds" => cfg.expiry_ms = number(value, 300)? * 1000,
                "delay_seconds" => cfg.delay_ms = number(value, 30)? * 1000,
                "cooldown_seconds" => cfg.cooldown_ms = number(value, 3600)? * 1000,
                "arrival_protection_seconds" => {
                    cfg.arrival_protection_ms = number(value, 60)? * 1000;
                }
                "max_pending" => {
                    cfg.max_pending = usize::try_from(number(value, 16)?).map_err(|_| BAD)?;
                    if cfg.max_pending == 0 {
                        return Err(BAD);
                    }
                }
                "safe_search_radius" => {
                    cfg.safe_search_radius = i32::try_from(number(value, 4)?).map_err(|_| BAD)?;
                }
                "partial_names" => cfg.partial_names = boolean(value)?,
                "buttons" => cfg.buttons = boolean(value)?,
                "toggle_subcommand" => cfg.toggle_subcommand = boolean(value)?,
                "safe_teleport" => cfg.safe_teleport = boolean(value)?,
                "water_safe" => cfg.water_safe = boolean(value)?,
                "center_on_block" => cfg.center_on_block = boolean(value)?,
                "dismount" => cfg.dismount = boolean(value)?,
                "safe_skip_modes" => {
                    cfg.safe_skip_modes = list(value)?;
                    if !cfg
                        .safe_skip_modes
                        .iter()
                        .all(|m| MODES.contains(&m.as_str()))
                    {
                        return Err(BAD);
                    }
                }
                "preference_api_plugins" => {
                    cfg.preference_api_plugins = list(value)?;
                    if !cfg.preference_api_plugins.iter().all(|p| plugin_name(p)) {
                        return Err(BAD);
                    }
                }
                _ => {
                    let command = key.strip_prefix("aliases.").ok_or(BAD)?;
                    if !COMMANDS.contains(&command) {
                        return Err(BAD);
                    }
                    let aliases = list(value)?;
                    if aliases.len() > MAX_ALIASES || !aliases.iter().all(|a| alias(a)) {
                        return Err(BAD);
                    }
                    cfg.aliases.insert(command.to_owned(), aliases);
                }
            }
        }
        let mut names: BTreeSet<&str> = COMMANDS.into_iter().collect();
        for name in cfg.aliases.values().flatten() {
            if !names.insert(name) {
                return Err(BAD);
            }
        }
        Ok(cfg)
    }
}

fn number(value: &str, limit: u64) -> Result<u64, &'static str> {
    value.parse::<u64>().ok().filter(|n| *n <= limit).ok_or(BAD)
}

fn boolean(value: &str) -> Result<bool, &'static str> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(BAD),
    }
}

/// A comma-separated list. An empty value is an empty list; an empty item is a mistake.
fn list(value: &str) -> Result<Vec<String>, &'static str> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(str::trim)
        .map(|item| {
            if item.is_empty() {
                Err(BAD)
            } else {
                Ok(item.to_owned())
            }
        })
        .collect()
}

fn alias(name: &str) -> bool {
    name.len() <= 32
        && !name.is_empty()
        && !RESERVED.contains(&name)
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn plugin_name(name: &str) -> bool {
    name.len() <= 64
        && !name.ends_with(['.', ' '])
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'_' | b'.' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_bounded_override() {
        let d = Config::parse("").unwrap();
        assert_eq!(d, Config::default());
        assert_eq!(
            (d.expiry_ms, d.delay_ms, d.cooldown_ms),
            (60_000, 3000, 30_000)
        );
        let c =
            Config::parse("expiry_seconds=300\ndelay_seconds=0\ncooldown_seconds=3600").unwrap();
        assert_eq!(
            (c.expiry_ms, c.delay_ms, c.cooldown_ms),
            (300_000, 0, 3_600_000)
        );
    }
    #[test]
    fn expiry_can_be_turned_off_explicitly() {
        assert_eq!(Config::parse("expiry_seconds=0").unwrap().expiry_ms, 0);
    }
    #[test]
    fn every_setting_is_read() {
        let c = Config::parse(
            "# all of them\n\
             max_pending=16\n\
             partial_names=true\n\
             buttons=false\n\
             toggle_subcommand=false\n\
             safe_teleport=false\n\
             safe_search_radius=4\n\
             water_safe=true\n\
             safe_skip_modes=creative,adventure\n\
             center_on_block=false\n\
             dismount=false\n\
             arrival_protection_seconds=60\n\
             aliases.tpa=call,tpask\n\
             aliases.tptoggle=\n\
             preference_api_plugins=menu-plugin, Other Plugin\n",
        )
        .unwrap();
        assert_eq!(c.max_pending, 16);
        assert!(c.partial_names && c.water_safe);
        assert!(!c.buttons && !c.toggle_subcommand && !c.safe_teleport);
        assert!(!c.center_on_block && !c.dismount);
        assert_eq!(c.safe_search_radius, 4);
        assert_eq!(c.safe_skip_modes, ["creative", "adventure"]);
        assert_eq!(c.arrival_protection_ms, 60_000);
        assert_eq!(c.aliases["tpa"], ["call", "tpask"]);
        assert!(c.aliases["tptoggle"].is_empty());
        assert_eq!(c.preference_api_plugins, ["menu-plugin", "Other Plugin"]);
        assert!(
            Config::parse("safe_skip_modes=")
                .unwrap()
                .safe_skip_modes
                .is_empty()
        );
    }
    #[test]
    fn untrusted_config_fails_closed_not_silent_defaults() {
        for s in [
            "expiry_seconds=301",
            "delay_seconds=31",
            "cooldown_seconds=3601",
            "expiry_seconds=-1",
            "expiry_seconds=NaN",
            "delay_seconds=1\ndelay_seconds=2",
            "unknown=1",
            "garbage",
            "expiry_seconds=18446744073709551615",
            "max_pending=0",
            "max_pending=17",
            "safe_search_radius=-1",
            "safe_search_radius=5",
            "arrival_protection_seconds=61",
            "partial_names=yes",
            "buttons=1",
            "safe_skip_modes=god",
            "aliases.unknown=x",
            "aliases.tpa=Upper",
            "aliases.tpa=a b",
            "aliases.tpa=ns:name",
            "aliases.tpa=tpaccept",
            "aliases.tpa=marketplace",
            "aliases.tpa=x,x",
            "aliases.tpa=x\naliases.tpdeny=x",
            "aliases.tpa=a,b,c,d,e,f,g,h,i",
            "preference_api_plugins=bad/name",
            "aliases.tpa=x\naliases.tpa=y",
        ] {
            assert!(Config::parse(s).is_err(), "{s}");
        }
        assert!(Config::parse(&" ".repeat(4097)).is_err());
    }
}
