# LetMeAFK

<p align="center"><img src="logo.png" alt="LetMeAFK logo" width="180"></p>

<p align="center">
  <a href="https://github.com/SkyeSlav/letmeafk/releases/latest"><img src="https://img.shields.io/github/v/release/SkyeSlav/letmeafk" alt="Latest release"></a>
  <a href="https://github.com/SkyeSlav/letmeafk/actions/workflows/release.yml"><img src="https://github.com/SkyeSlav/letmeafk/actions/workflows/release.yml/badge.svg" alt="Release build status"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/SkyeSlav/letmeafk" alt="License: MIT"></a>
</p>

Keeps you from going AFK. Toggle it with `/lmafk` or **F8** and it moves, jumps, sneaks, swings, punches and uses the held item on timers, walks a WASD cycle, turns the camera, swaps the hotbar and offhand, opens the inventory, eats when hungry, chats on a schedule or sends a small command instead, and logs out after a set time.

> AFK automation is against the rules on many servers. This mod moves, clicks, chats and sends commands for you — exactly the kind of input anticheats watch for — so using it can get you flagged, kicked, or banned, temporarily or permanently, even on a server where it seems to work fine. It does not hide itself from detection, and there is no way to use it safely on a server that forbids it. Check each server's rules before you turn it on, and use it at your own risk.

## Install

1. Download `letmeafk-1.0.0.hmod` from the latest release.
2. On Starling 0.9.0, import the `.hmod` from the Content Library. On the Horizon launcher, put it in the `horizon-mods` folder of your game directory and enable it in `horizon-mods.txt` (`mod.letmeafk:true`).
3. Start the game. It is off by default — turn it on with `/lmafk` or F8.

Build it from this repo if you want (needs Rust 1.85+ and the `cargo-horizon-mod` tool; the tool fetches the pinned SDK itself):

```sh
cargo install --locked --git https://github.com/horizon-rust/horizon cargo-horizon-mod
cd hmod && cargo horizon-mod build   # target/horizon-mod/letmeafk.hmod
```

## Commands

All commands start with `/lmafk`:

- `/lmafk` — toggle on/off
- `/lmafk status` — show what is on and how long you have been AFK
- `/lmafk reset` — restore the original AntiAfk defaults
- `/lmafk help` — list every subcommand
- `/lmafk interval set <secs>` — fixed delay between actions
- `/lmafk interval random <min> <max>` — random delay between actions
- `/lmafk interval` — show the current interval
- `/lmafk swing`, `/lmafk jump`, `/lmafk sneak`, `/lmafk movement` — toggle one feature
- `/lmafk punch` — attack whatever is under the crosshair, on the timed action. It can hit mobs, players and blocks, so only use it where that is safe.
- `/lmafk use` — use the held item, on the timed action. It does what a click does: place, throw, draw — so hold something harmless.
- `/lmafk spin [<secs>]` — toggle spinning, optionally at a set speed
- `/lmafk mousemovement [<range>]` — toggle camera movement, optionally with a set range
- `/lmafk eat [<level>]` — toggle auto-eat, optionally at a set hunger level
- `/lmafk hotbar`, `/lmafk offhand`, `/lmafk inventory`, `/lmafk jitter`, `/lmafk pause` — toggle one feature
- `/lmafk chat [add <message...> | list | clear]` — manage the auto-chat messages
- `/lmafk command` — send a small command now and then instead of a chat message; `/lmafk cmd add <command> | remove <number> | list | clear` — the command list; `/lmafk command min <secs>` / `max <secs>` — how often. The default list is commands that do not exist (`/letmeafk`, `/zzz`, `/afk-check`), so the server just answers "Unknown command" to you alone — nothing shows in public chat.
- `/lmafk disconnect [<radius>]` — toggle disconnecting when a player gets close
- `/lmafk damage` — toggle disconnecting the moment you take damage
- `/lmafk ignore add <name> | remove <name> | list` — players the disconnect check ignores
- `/lmafk logout <minutes>` / `/lmafk logout off` — automatic logout timer
- `/lmafk hud` — toggle the `[AFK] mm:ss` display
- `/lmafk random [<min> <max>]` — toggle random action intervals

## Settings

Everything above is also in the mod's settings page, grouped so each range sits under its feature: actions, camera, timing, eating, items, chat, safety, display — 41 settings in total. The toggle key (F8 by default) can be rebound from the keybinds screen.

Out of the box it matches the original AntiAfk: only auto-jump (every 5 seconds) and auto-eat are on. Turn the rest on one by one with `/lmafk <feature>` or the settings page. If you installed the earlier test build, run `/lmafk reset` once — it kept the busier settings.

## Notes

- It never reconnects on its own: if you get disconnected, rejoin by hand.
- It disconnects you itself when a non-ignored player comes within your set radius, and when you take damage if that is on.
- It warns in chat at 80% of the logout timer, then logs out at 100%.
- It keeps working behind menus: jumping, swinging, eating, the camera and chat all continue while a screen is open. Only the movement keys hold still, so it never walks you anywhere while you are in a menu.

## Credit

Inspired by AntiAfk for Fabric by CIOCOLATA47 (MIT). This is a clean-room rewrite for Starling/Horizon — no code from the original is used.
