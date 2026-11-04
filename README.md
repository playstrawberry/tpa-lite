# TPA Lite

Teleport requests for Play Strawberry servers. A player asks to teleport to another player, or
invites another player to come to him; the other player accepts or denies with a command or a
click in chat.

## Commands

| Command | What it does | Permission |
| --- | --- | --- |
| `/tpa <player>` | Asks to teleport to that player | `tpa-lite:use` |
| `/tpahere <player>` | Invites that player to teleport to you | `tpa-lite:here` |
| `/tpaccept [player\|*]` | Accepts the newest request, that player's, or all | `tpa-lite:accept` |
| `/tpdeny [player\|*]` | Denies the newest request, that player's, or all | `tpa-lite:deny` |
| `/tpacancel [player]` | Cancels your requests, or the one to that player | `tpa-lite:cancel` |
| `/tptoggle` (also `/tpa toggle`) | Turns incoming requests on or off; kept across restarts | `tpa-lite:toggle` |

## Permissions

Every permission is given to everyone by default. Take one away to turn that command off, for
example to allow `/tpa`, `/tpaccept` and `/tpdeny` and nothing else.

## How requests work

- A request expires after 60 seconds. When it is accepted, the traveller waits 3 seconds; moving,
  taking damage, changing world or another teleport cancels the wait.
- A sender waits 30 seconds between two requests.
- The plugin lands the player only on a safe spot: a solid block with two free blocks above it.
  If the spot is not safe it looks for the nearest safe one close by; if there is none, nothing
  happens.
- Players only see and find players they can see. A hidden player and a player who is not online
  get the same answer.
- Death, leaving the server or a reload throws the requests away.

## Settings

All settings are optional. Copy [`tpa.properties`](tpa.properties) to
`plugins/data/tpa-lite/tpa.properties` and change what you need, then restart or reload.

| Setting | Default | Allowed |
| --- | --- | --- |
| `expiry_seconds` | 60 | 0 (never expires) to 300 |
| `delay_seconds` | 3 | 0 to 30 |
| `cooldown_seconds` | 30 | 0 to 3600 |
| `arrival_protection_seconds` | 0 | 0 (off) to 60: no damage taken or dealt just after arriving |
| `max_pending` | 5 | 1 to 16 requests waiting on one player |
| `partial_names` | false | the start of a name also finds a player |
| `buttons` | true | clickable accept and deny in chat |
| `toggle_subcommand` | true | whether `/tpa toggle` exists |
| `safe_teleport` | true | false skips the safe-spot check |
| `safe_search_radius` | 3 | 0 to 4 blocks |
| `water_safe` | false | landing in water counts as safe |
| `safe_skip_modes` | creative,spectator | game modes that skip the safe-spot check |
| `center_on_block` | true | land in the middle of the block |
| `dismount` | true | get off a mount before teleporting |
| `aliases.<command>` | none | extra names for a command, for example `aliases.tpa=tpask` |
| `preference_api_plugins` | none | plugins allowed to read and change a player's on/off choice |

A value out of range stops the plugin from loading and the console names it.

## Messages

Every text is in `lang/en.lang`, `fr.lang`, `es.lang` and `tr.lang`; each player reads his game
language, with English as the fallback. To change a text, put `key=text` lines in
`plugins/data/tpa-lite/lang/<code>.lang`. Colours are `&` codes and `&#rrggbb`; an empty text
sends nothing.

## For other plugins

A settings menu plugin listed in `preference_api_plugins` can read and change a player's choice
with plugin messages:

| Message | Answer |
| --- | --- |
| `requests-enabled <uuid>` | `ok true` or `ok false` |
| `set-requests-enabled <uuid> <true\|false>` | the choice after the call |

## Install on a Play Strawberry server

1. Add TPA Lite to your server in the Play Strawberry panel.
2. Restart the server. The plugin keeps its files in `plugins/data/tpa-lite/`.
3. Optional: add `tpa.properties` there to change the settings.

Play Strawberry servers load only plugins signed by Play Strawberry, so a file you build yourself
from this source does not load on them.

## Build from source

You need Rust with the `wasm32-wasip2` target and the Strawberry plugin SDK
(`strawberry-plugin-api`), which Play Strawberry gives to its plugin developers.
`Cargo.toml` takes it from the SDK registry named `strawberry`; add that registry to
your Cargo configuration (for example `~/.cargo/config.toml`):

```toml
[registries.strawberry]
index = "sparse+https://sdk.playstrawberry.net/index/"
```

```text
cargo test
cargo build --release --target wasm32-wasip2
```

The plugin file is `target/wasm32-wasip2/release/tpa_lite.wasm`.

## License

Copyright Strawberry Interactive LLC. The license text is not published yet; until it is, all
rights are reserved.
