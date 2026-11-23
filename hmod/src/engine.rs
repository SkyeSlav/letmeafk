//! LetMeAFK for Horizon (native .hmod): keeps the player from going
//! AFK by triggering actions on timers — jump, sneak, swing, punch,
//! use, a WASD walk cycle, camera moves, spin, eating, hotbar
//! shuffling, offhand swaps, opening the inventory, chat messages,
//! auto-commands, auto-disconnect when another player gets close,
//! disconnect on damage, and auto-logout after a set time.
//!
//! The game thread calls [`tick`] once per client tick, and [`command`]
//! for the mod's own commands. All state lives behind one mutex; the
//! hooks in `lib.rs` do the game's I/O (settings files, packets, chat
//! lines, hotbar selection, rotation).
//!
//! The idea is the same one as the well-known anti-AFK mods: do
//! something at intervals so the server never sees an idle player.
//! This is a clean-room reimplementation in Rust — no code from any
//! other mod is used.
//!
//! The mod does nothing until it is toggled on (`/lmafk` or the
//! toggle keybind). Toggling it off stops everything and clears the
//! movement keys, so the player is in control again immediately.
//!
//! How this build differs from the Fabric original: there is no
//! auto-reconnect in 1.0.0 — the mod API has no way for a mod to
//! connect to a server (no `client_connect`), so reconnecting waits
//! on an upstream addition. The last server's address is still
//! recorded on disconnect, so reconnect is one call site when that
//! lands. There is also no in-game config screen in 1.0.0: every
//! value lives on the settings page or in `/lmafk` commands.

use std::sync::{LazyLock, Mutex, MutexGuard};

pub const ID: &str = "letmeafk";
pub const NAME: &str = "LetMeAFK";
pub const VERSION: &str = "1.0.0";
pub const DESCRIPTION: &str = "Keeps you from going AFK: timed jumps, sneak, swing, punch, use, a WASD walk cycle, camera moves, spin, auto-eat, hotbar shuffling, offhand swaps, inventory opens, chat messages, auto-commands, auto-disconnect, disconnect on damage and auto-logout. Many servers forbid AFK automation — check the rules first.";

/// The WASD walk cycle: this many ticks walking one direction, then
/// this many ticks standing still, per phase (forward, right, back,
/// left), as the original does.
const WALK_TICKS: u64 = 40;
const WALK_PAUSE_TICKS: u64 = 10;
/// The mouse-movement anchor picks a new target this often (ticks).
const MOUSE_RETARGET_TICKS: u64 = 50;
/// A mouse-jitter nudge lasts this many ticks before it is re-rolled.
const JITTER_MIN_TICKS: u64 = 2;
const JITTER_MAX_TICKS: u64 = 5;
/// How far a jitter nudge moves the view (degrees): the original's
/// 1.5-degree strength, halved for yaw and quartered for pitch.
const JITTER_YAW_STRENGTH: f32 = 0.75;
const JITTER_PITCH_STRENGTH: f32 = 0.375;
/// The spin's pitch wave: this many degrees either way.
const SPIN_PITCH_WAVE: f32 = 20.0;
/// Eating takes this many ticks (the original holds use for 40).
const EAT_TICKS: u64 = 40;
/// The auto-disconnect check runs this often (ticks).
const DISCONNECT_CHECK_TICKS: u64 = 10;
/// The random pause rolls this often: a 1-in-200 chance per tick,
/// the original's 0.5%.
/// A random pause lasts between these many ticks (20-59, the
/// original's 20 + nextInt(40)).
const PAUSE_CHANCE_DENOMINATOR: u64 = 200;
const PAUSE_MIN_TICKS: u64 = 20;
const PAUSE_MAX_TICKS: u64 = 59;

/// Settings keys, one per key under the mod's data folder. The two
/// lists live in data files of the same names.
const KEY_JUMP: &str = "jump";
const KEY_SNEAK: &str = "sneak";
const KEY_SWING: &str = "swing";
const KEY_MOVEMENT: &str = "movement";
const KEY_SPIN: &str = "spin";
const KEY_MOUSE_MOVEMENT: &str = "mouse_movement";
const KEY_MOUSE_JITTER: &str = "mouse_jitter";
const KEY_RANDOM_PAUSE: &str = "random_pause";
const KEY_RANDOM_INTERVAL: &str = "random_interval";
const KEY_AUTO_EAT: &str = "auto_eat";
const KEY_RANDOM_HOTBAR: &str = "random_hotbar";
const KEY_OFFHAND_SWAP: &str = "offhand_swap";
const KEY_OPEN_INVENTORY: &str = "open_inventory";
const KEY_AUTO_CHAT: &str = "auto_chat";
const KEY_AUTO_COMMAND: &str = "auto_command";
const KEY_PUNCH: &str = "punch";
const KEY_USE_ITEM: &str = "use_item";
const KEY_DISCONNECT_ON_DAMAGE: &str = "disconnect_on_damage";
const KEY_AUTO_DISCONNECT: &str = "auto_disconnect";
const KEY_AUTO_LOGOUT: &str = "auto_logout";
const KEY_HUD: &str = "hud";
const KEY_ACTION_DELAY: &str = "action_delay";
const KEY_MIN_SECS: &str = "min_secs";
const KEY_MAX_SECS: &str = "max_secs";
const KEY_SPIN_SPEED: &str = "spin_speed";
const KEY_MOUSE_RANGE: &str = "mouse_range";
const KEY_EAT_BELOW: &str = "eat_below";
const KEY_HOTBAR_MIN: &str = "hotbar_min";
const KEY_HOTBAR_MAX: &str = "hotbar_max";
const KEY_OFFHAND_MIN: &str = "offhand_min";
const KEY_OFFHAND_MAX: &str = "offhand_max";
const KEY_OFFHAND_HOLD: &str = "offhand_hold";
const KEY_INV_MIN: &str = "inv_min";
const KEY_INV_MAX: &str = "inv_max";
const KEY_INV_HOLD: &str = "inv_hold";
const KEY_CHAT_MIN: &str = "chat_min";
const KEY_CMD_MIN: &str = "cmd_min";
const KEY_CMD_MAX: &str = "cmd_max";
const KEY_CHAT_MAX: &str = "chat_max";
const KEY_DISCONNECT_RADIUS: &str = "disconnect_radius";
const KEY_LOGOUT_MINUTES: &str = "logout_minutes";
const KEY_CHAT_MESSAGES: &str = "chat_messages";
const KEY_COMMANDS: &str = "commands";
const KEY_IGNORED_PLAYERS: &str = "ignored_players";

/// The settings page's choices for each range, in seconds (or the
/// unit the setting uses). The button cycles to the next value.
const ACTION_DELAY_CHOICES: [f64; 10] = [1.0, 2.0, 3.0, 5.0, 7.0, 10.0, 15.0, 20.0, 30.0, 60.0];
const MIN_SECS_CHOICES: [f64; 5] = [1.0, 2.0, 3.0, 5.0, 7.0];
const MAX_SECS_CHOICES: [f64; 6] = [3.0, 5.0, 7.0, 10.0, 15.0, 30.0];
const SPIN_SPEED_CHOICES: [f64; 5] = [3.0, 5.0, 9.0, 15.0, 30.0];
const MOUSE_RANGE_CHOICES: [f64; 5] = [15.0, 30.0, 45.0, 60.0, 90.0];
const EAT_BELOW_CHOICES: [f64; 6] = [6.0, 10.0, 14.0, 16.0, 18.0, 20.0];
const HOTBAR_MIN_CHOICES: [f64; 4] = [1.0, 3.0, 5.0, 10.0];
const HOTBAR_MAX_CHOICES: [f64; 5] = [5.0, 10.0, 15.0, 30.0, 60.0];
const OFFHAND_MIN_CHOICES: [f64; 4] = [5.0, 10.0, 15.0, 20.0];
const OFFHAND_MAX_CHOICES: [f64; 5] = [15.0, 30.0, 45.0, 60.0, 120.0];
const OFFHAND_HOLD_CHOICES: [f64; 4] = [1.0, 3.0, 5.0, 10.0];
const INV_MIN_CHOICES: [f64; 3] = [10.0, 20.0, 30.0];
const INV_MAX_CHOICES: [f64; 5] = [25.0, 30.0, 60.0, 120.0, 300.0];
const INV_HOLD_CHOICES: [f64; 5] = [1.0, 2.0, 3.0, 5.0, 10.0];
const CHAT_MIN_CHOICES: [f64; 3] = [30.0, 60.0, 120.0];
const CHAT_MAX_CHOICES: [f64; 5] = [60.0, 120.0, 180.0, 300.0, 600.0];
const CMD_MIN_CHOICES: [f64; 3] = [30.0, 60.0, 120.0];
const CMD_MAX_CHOICES: [f64; 5] = [60.0, 120.0, 180.0, 300.0, 600.0];
const DISCONNECT_RADIUS_CHOICES: [f64; 5] = [4.0, 8.0, 10.0, 16.0, 32.0];
const LOGOUT_MINUTES_CHOICES: [f64; 6] = [0.0, 5.0, 10.0, 30.0, 60.0, 120.0];

/// The mod's settings.
#[derive(Clone, Debug)]
pub struct Config {
    pub jump: bool,
    pub sneak: bool,
    pub swing: bool,
    pub movement: bool,
    pub spin: bool,
    pub mouse_movement: bool,
    pub mouse_jitter: bool,
    pub random_pause: bool,
    pub random_interval: bool,
    pub auto_eat: bool,
    pub random_hotbar: bool,
    pub offhand_swap: bool,
    pub open_inventory: bool,
    pub auto_chat: bool,
    pub auto_command: bool,
    pub punch: bool,
    pub use_item: bool,
    pub auto_disconnect: bool,
    pub disconnect_on_damage: bool,
    pub auto_logout: bool,
    pub hud: bool,
    /// Seconds between timed actions (jump / swing / sneak toggle).
    pub action_delay: f64,
    /// Random-interval range, in seconds.
    pub min_secs: f64,
    pub max_secs: f64,
    /// Seconds per full 360° spin.
    pub spin_speed: f64,
    /// Mouse-movement range around the anchor, in degrees.
    pub mouse_range: f64,
    /// Eat when hunger is at or below this (out of 20).
    pub eat_below: u8,
    /// Hotbar-switch interval range, in seconds.
    pub hotbar_min: f64,
    pub hotbar_max: f64,
    /// Offhand-swap interval range, in seconds.
    pub offhand_min: f64,
    pub offhand_max: f64,
    /// Seconds the item stays in the offhand before swapping back.
    pub offhand_hold: f64,
    /// Inventory-open interval range, in seconds.
    pub inv_min: f64,
    pub inv_max: f64,
    /// Seconds the inventory stays open.
    pub inv_hold: f64,
    /// Chat-message interval range, in seconds.
    pub chat_min: f64,
    pub chat_max: f64,
    /// Auto-command interval range, in seconds.
    pub cmd_min: f64,
    pub cmd_max: f64,
    /// Disconnect when a player gets within this many blocks.
    pub disconnect_radius: f64,
    /// Auto-logout after this many minutes (0 = off).
    pub logout_minutes: f64,
    /// Chat messages, sent at random; semicolon-separated in storage.
    pub chat_messages: Vec<String>,
    /// Commands, sent at random; semicolon-separated in storage.
    /// Each is sent as a command (a leading slash is added when the
    /// entry has none), so an entry that is not a real command just
    /// gets the server's "Unknown command" answer, to the player only.
    pub commands: Vec<String>,
    /// Players the auto-disconnect ignores; semicolon-separated.
    pub ignored_players: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        // The Fabric original's defaults, verified against the
        // shipped AntiAfk 1.2.9 jar: out of the box only jump (every
        // 5s) and auto-eat are on. The first test build had swing,
        // movement, jitter, random pauses, random intervals and
        // auto-disconnect on as well, so toggling it on marched the
        // player around swinging — far busier than the original. The
        // chat messages are the original's defaults too.
        Self {
            jump: true,
            sneak: false,
            swing: false,
            movement: false,
            spin: false,
            mouse_movement: false,
            mouse_jitter: false,
            random_pause: false,
            random_interval: false,
            auto_eat: true,
            random_hotbar: false,
            offhand_swap: false,
            open_inventory: false,
            auto_chat: false,
            auto_command: false,
            punch: false,
            use_item: false,
            auto_disconnect: false,
            disconnect_on_damage: false,
            auto_logout: false,
            hud: true,
            action_delay: 5.0,
            min_secs: 3.0,
            max_secs: 7.0,
            spin_speed: 5.0,
            mouse_range: 60.0,
            eat_below: 16,
            hotbar_min: 5.0,
            hotbar_max: 15.0,
            offhand_min: 15.0,
            offhand_max: 45.0,
            offhand_hold: 3.0,
            inv_min: 10.0,
            inv_max: 25.0,
            inv_hold: 2.0,
            chat_min: 30.0,
            chat_max: 120.0,
            cmd_min: 30.0,
            cmd_max: 120.0,
            disconnect_radius: 10.0,
            logout_minutes: 30.0,
            chat_messages: vec![
                "Hey!".to_string(),
                "Still here!".to_string(),
                "Just farming!".to_string(),
            ],
            ignored_players: Vec::new(),
            commands: vec![
                "/letmeafk".to_string(),
                "/zzz".to_string(),
                "/afk-check".to_string(),
            ],
        }
    }
}

impl Config {
    fn load(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let bool = |k: &str, default: bool| match get(k).as_deref() {
            Some("true") => true,
            Some("false") => false,
            _ => default,
        };
        let num = |k: &str, default: f64| {
            get(k)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(default)
        };
        // A missing key means "never stored": fall back to the
        // default list. A stored empty value parses to an empty list,
        // so an explicit `clear` still sticks.
        let list = |k: &str, default: Vec<String>| {
            get(k)
                .map(|v| {
                    v.split(';')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or(default)
        };
        let clamp = |v: f64, lo: f64, hi: f64| v.clamp(lo, hi);
        let action_delay = clamp(num(KEY_ACTION_DELAY, d.action_delay), 0.5, 60.0);
        let min_secs = clamp(num(KEY_MIN_SECS, d.min_secs), 0.5, 60.0);
        let max_secs = clamp(num(KEY_MAX_SECS, d.max_secs), min_secs, 60.0);
        let hotbar_min = clamp(num(KEY_HOTBAR_MIN, d.hotbar_min), 1.0, 120.0);
        let offhand_min = clamp(num(KEY_OFFHAND_MIN, d.offhand_min), 1.0, 300.0);
        let inv_min = clamp(num(KEY_INV_MIN, d.inv_min), 1.0, 300.0);
        let chat_min = clamp(num(KEY_CHAT_MIN, d.chat_min), 1.0, 1200.0);
        let cmd_min = clamp(num(KEY_CMD_MIN, d.cmd_min), 1.0, 1200.0);
        Self {
            jump: bool(KEY_JUMP, d.jump),
            sneak: bool(KEY_SNEAK, d.sneak),
            swing: bool(KEY_SWING, d.swing),
            movement: bool(KEY_MOVEMENT, d.movement),
            spin: bool(KEY_SPIN, d.spin),
            mouse_movement: bool(KEY_MOUSE_MOVEMENT, d.mouse_movement),
            mouse_jitter: bool(KEY_MOUSE_JITTER, d.mouse_jitter),
            random_pause: bool(KEY_RANDOM_PAUSE, d.random_pause),
            random_interval: bool(KEY_RANDOM_INTERVAL, d.random_interval),
            auto_eat: bool(KEY_AUTO_EAT, d.auto_eat),
            random_hotbar: bool(KEY_RANDOM_HOTBAR, d.random_hotbar),
            offhand_swap: bool(KEY_OFFHAND_SWAP, d.offhand_swap),
            open_inventory: bool(KEY_OPEN_INVENTORY, d.open_inventory),
            auto_chat: bool(KEY_AUTO_CHAT, d.auto_chat),
            auto_command: bool(KEY_AUTO_COMMAND, d.auto_command),
            punch: bool(KEY_PUNCH, d.punch),
            use_item: bool(KEY_USE_ITEM, d.use_item),
            auto_disconnect: bool(KEY_AUTO_DISCONNECT, d.auto_disconnect),
            disconnect_on_damage: bool(KEY_DISCONNECT_ON_DAMAGE, d.disconnect_on_damage),
            auto_logout: bool(KEY_AUTO_LOGOUT, d.auto_logout),
            hud: bool(KEY_HUD, d.hud),
            action_delay,
            min_secs,
            max_secs,
            spin_speed: clamp(num(KEY_SPIN_SPEED, d.spin_speed), 2.0, 30.0),
            mouse_range: clamp(num(KEY_MOUSE_RANGE, d.mouse_range), 5.0, 90.0),
            eat_below: clamp(num(KEY_EAT_BELOW, d.eat_below as f64), 1.0, 20.0) as u8,
            hotbar_min,
            hotbar_max: clamp(num(KEY_HOTBAR_MAX, d.hotbar_max), hotbar_min, 300.0),
            offhand_min,
            offhand_max: clamp(num(KEY_OFFHAND_MAX, d.offhand_max), offhand_min, 600.0),
            offhand_hold: clamp(num(KEY_OFFHAND_HOLD, d.offhand_hold), 1.0, 30.0),
            inv_min,
            inv_max: clamp(num(KEY_INV_MAX, d.inv_max), inv_min, 600.0),
            inv_hold: clamp(num(KEY_INV_HOLD, d.inv_hold), 1.0, 30.0),
            chat_min,
            chat_max: clamp(num(KEY_CHAT_MAX, d.chat_max), chat_min, 3600.0),
            cmd_min,
            cmd_max: clamp(num(KEY_CMD_MAX, d.cmd_max), cmd_min, 3600.0),
            disconnect_radius: clamp(num(KEY_DISCONNECT_RADIUS, d.disconnect_radius), 1.0, 64.0),
            logout_minutes: clamp(num(KEY_LOGOUT_MINUTES, d.logout_minutes), 0.0, 240.0),
            chat_messages: list(KEY_CHAT_MESSAGES, d.chat_messages.clone()),
            commands: list(KEY_COMMANDS, d.commands.clone()),
            ignored_players: list(KEY_IGNORED_PLAYERS, d.ignored_players.clone()),
        }
    }

    fn stored(&self) -> Vec<(&'static str, String)> {
        let b = |v: bool| v.to_string();
        let n = |v: f64| {
            if v.fract() == 0.0 {
                format!("{}", v as u64)
            } else {
                format!("{v}")
            }
        };
        vec![
            (KEY_JUMP, b(self.jump)),
            (KEY_SNEAK, b(self.sneak)),
            (KEY_SWING, b(self.swing)),
            (KEY_MOVEMENT, b(self.movement)),
            (KEY_SPIN, b(self.spin)),
            (KEY_MOUSE_MOVEMENT, b(self.mouse_movement)),
            (KEY_MOUSE_JITTER, b(self.mouse_jitter)),
            (KEY_RANDOM_PAUSE, b(self.random_pause)),
            (KEY_RANDOM_INTERVAL, b(self.random_interval)),
            (KEY_AUTO_EAT, b(self.auto_eat)),
            (KEY_RANDOM_HOTBAR, b(self.random_hotbar)),
            (KEY_OFFHAND_SWAP, b(self.offhand_swap)),
            (KEY_OPEN_INVENTORY, b(self.open_inventory)),
            (KEY_AUTO_CHAT, b(self.auto_chat)),
            (KEY_AUTO_COMMAND, b(self.auto_command)),
            (KEY_PUNCH, b(self.punch)),
            (KEY_USE_ITEM, b(self.use_item)),
            (KEY_AUTO_DISCONNECT, b(self.auto_disconnect)),
            (KEY_DISCONNECT_ON_DAMAGE, b(self.disconnect_on_damage)),
            (KEY_AUTO_LOGOUT, b(self.auto_logout)),
            (KEY_HUD, b(self.hud)),
            (KEY_ACTION_DELAY, n(self.action_delay)),
            (KEY_MIN_SECS, n(self.min_secs)),
            (KEY_MAX_SECS, n(self.max_secs)),
            (KEY_SPIN_SPEED, n(self.spin_speed)),
            (KEY_MOUSE_RANGE, n(self.mouse_range)),
            (KEY_EAT_BELOW, n(self.eat_below as f64)),
            (KEY_HOTBAR_MIN, n(self.hotbar_min)),
            (KEY_HOTBAR_MAX, n(self.hotbar_max)),
            (KEY_OFFHAND_MIN, n(self.offhand_min)),
            (KEY_OFFHAND_MAX, n(self.offhand_max)),
            (KEY_OFFHAND_HOLD, n(self.offhand_hold)),
            (KEY_INV_MIN, n(self.inv_min)),
            (KEY_INV_MAX, n(self.inv_max)),
            (KEY_INV_HOLD, n(self.inv_hold)),
            (KEY_CHAT_MIN, n(self.chat_min)),
            (KEY_CHAT_MAX, n(self.chat_max)),
            (KEY_CMD_MIN, n(self.cmd_min)),
            (KEY_CMD_MAX, n(self.cmd_max)),
            (KEY_DISCONNECT_RADIUS, n(self.disconnect_radius)),
            (KEY_LOGOUT_MINUTES, n(self.logout_minutes)),
            (KEY_CHAT_MESSAGES, self.chat_messages.join(";")),
            (KEY_COMMANDS, self.commands.join(";")),
            (KEY_IGNORED_PLAYERS, self.ignored_players.join(";")),
        ]
    }

    /// Whether `name` is on the ignore list, case-insensitively.
    fn is_ignored(&self, name: &str) -> bool {
        self.ignored_players
            .iter()
            .any(|n| n.eq_ignore_ascii_case(name))
    }
}

/// The movement keys the adapter applies through the `movement_input`
/// hook this tick. While the mod is toggled on, it owns all of them:
/// the original sets the key states outright, so the player cannot
/// sneak or walk while it is active either.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sneak: bool,
}

/// What the adapter saw this tick.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// The player's current look (yaw, pitch), in degrees.
    pub yaw: f32,
    pub pitch: f32,
    /// Whether the player is on the ground (jump is gated on it).
    pub on_ground: bool,
    /// Whether any screen is open (movement stops, and the
    /// inventory / hotbar / offhand actions skip, while one is).
    pub screen_open: bool,
    /// Hunger level out of 20, or -1 when it cannot be read.
    pub food_level: i32,
    /// Health in half-hearts, or -1.0 when it cannot be read.
    pub health: f32,
    /// The selected hotbar slot.
    pub selected_slot: usize,
    /// Hotbar slots holding food, in slot order.
    pub food_slots: Vec<usize>,
    /// The local player's position.
    pub player_pos: [f64; 3],
    /// Other players the client sees, with their positions. The
    /// adapter has already excluded the local player.
    pub nearby: Vec<(String, [f64; 3])>,
    /// Whether the player is currently using an item (eating).
    pub using_item: bool,
}

/// What a tick does: the movement keys, the look to set, and any
/// one-shot actions. The adapter applies these and nothing else, so
/// a tick that says nothing leaves the game alone.
#[derive(Default, Debug)]
pub struct TickOut {
    /// The movement keys for this tick.
    pub keys: Keys,
    /// The look to set this tick, if the mod is turning the view.
    pub rotation: Option<(f32, f32)>,
    /// Swing the hand (one `minecraft:swing` packet).
    pub swing: bool,
    /// Punch: one click of the attack mapping — the game attacks
    /// whatever is under the crosshair, as the player's own click
    /// would. Dropped while a screen is open.
    pub punch: bool,
    /// Use: one click of the use mapping — the held item's own use,
    /// as the player's own click would (place, throw, draw...).
    /// Dropped while a screen is open.
    pub use_item: bool,
    /// Select this hotbar slot first (eat / random hotbar).
    pub select_slot: Option<u8>,
    /// Hold the use key (eat start) or release it (eat end). None
    /// means leave the key alone — the adapter must not release the
    /// player's own use key every tick.
    pub hold_use: Option<bool>,
    /// Open the inventory.
    pub open_inventory: bool,
    /// Close the open screen (the inventory this mod opened).
    pub close_screen: bool,
    /// Swap the main hand with the offhand (one packet).
    pub swap_offhand: bool,
    /// Send this chat message.
    pub send_chat: Option<String>,
    /// Disconnect with this reason.
    pub disconnect: Option<String>,
    /// Chat lines for the player.
    pub lines: Vec<String>,
}

struct State {
    config: Config,
    /// Settings writes the adapter still has to persist.
    store: Vec<(String, String)>,
    tick: u64,
    /// Whether the mod is toggled on (keybind / `/lmafk toggle`).
    active: bool,
    /// Ticks since toggled on, for the HUD and the logout timer.
    afk_ticks: u64,
    /// Whether the 80% logout warning was already shown.
    logout_warned: bool,
    /// The tick the next timed action fires.
    action_due: u64,
    /// Ticks of random pause left (movement + timed actions stop).
    pause_remaining: u64,
    /// The WASD cycle: 0 forward, 1 right, 2 back, 3 left.
    walk_phase: usize,
    /// Ticks spent in the current walk phase.
    walk_tick: u64,
    /// The sneak toggle's current state.
    sneak_on: bool,
    /// Spin state: the smoothed yaw velocity and the yaw itself.
    spin_vel: f32,
    spin_yaw: f32,
    /// Mouse movement: the anchor the targets stay near, the current
    /// target, and the ticks until a new target is picked.
    mouse_anchor: Option<(f32, f32)>,
    mouse_target: (f32, f32),
    mouse_timer: u64,
    /// The current jitter offset, re-rolled every few ticks.
    jitter_offset: (f32, f32),
    jitter_timer: u64,
    /// Eating: whether a use is in progress, for how many ticks, and
    /// which slot to restore afterwards.
    eating: bool,
    eat_ticks: u64,
    eat_restore: usize,
    /// The tick the next hotbar switch / offhand swap / inventory
    /// open / chat message fires.
    hotbar_due: u64,
    offhand_due: u64,
    /// Ticks the item stays in the offhand before swapping back.
    offhand_hold_left: u64,
    inv_due: u64,
    /// Ticks the inventory stays open; 0 means not open by this mod.
    inv_hold_left: u64,
    /// Whether the open screen is the inventory this mod opened.
    inv_open_by_us: bool,
    chat_due: u64,
    /// The tick the next auto-command fires.
    cmd_due: u64,
    /// Health on the last tick with a readable value (-1.0 when
    /// none yet); a drop from it is damage.
    last_health: f32,
    /// The movement keys and look the adapter applies this tick.
    keys: Keys,
    rotation: Option<(f32, f32)>,
    /// The deterministic RNG's state (xorshift64star).
    rng: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            config: Config::default(),
            store: Vec::new(),
            tick: 0,
            active: false,
            afk_ticks: 0,
            logout_warned: false,
            action_due: 0,
            pause_remaining: 0,
            walk_phase: 0,
            walk_tick: 0,
            sneak_on: false,
            spin_vel: 0.0,
            spin_yaw: 0.0,
            mouse_anchor: None,
            mouse_target: (0.0, 0.0),
            mouse_timer: 0,
            jitter_offset: (0.0, 0.0),
            jitter_timer: 0,
            eating: false,
            eat_ticks: 0,
            eat_restore: 0,
            hotbar_due: 0,
            offhand_due: 0,
            offhand_hold_left: 0,
            inv_due: 0,
            inv_hold_left: 0,
            inv_open_by_us: false,
            chat_due: 0,
            cmd_due: 0,
            last_health: -1.0,
            keys: Keys::default(),
            rotation: None,
            rng: 0x9E3779B97F4A7C15,
        }
    }
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read the settings from the game's files.
pub fn load(get: &dyn Fn(&str) -> Option<String>) {
    state().config = Config::load(get);
}

/// Settings writes the adapter still has to persist.
pub fn take_store() -> Vec<(String, String)> {
    std::mem::take(&mut state().store)
}

/// Whether the mod is toggled on.
pub fn is_active() -> bool {
    state().active
}

/// The movement keys the adapter applies through `movement_input`.
pub fn wanted_keys() -> Keys {
    state().keys
}

/// The look the adapter sets this tick, if the mod is turning the view.
pub fn wanted_rotation() -> Option<(f32, f32)> {
    state().rotation
}

/// The HUD line (`[AFK] mm:ss`, plus the logout countdown), or None
/// when the HUD is off or the mod is toggled off.
pub fn hud_line() -> Option<String> {
    let s = state();
    if !s.active || !s.config.hud {
        return None;
    }
    let secs = s.afk_ticks / 20;
    let mut line = format!("[AFK] {:02}:{:02}", secs / 60, secs % 60);
    if s.config.auto_logout && s.config.logout_minutes > 0.0 {
        let total = (s.config.logout_minutes * 1200.0) as u64;
        if total > 0 {
            let pct = (s.afk_ticks * 100)
                .checked_div(total)
                .unwrap_or(100)
                .min(100);
            let left = total.saturating_sub(s.afk_ticks);
            line.push_str(&format!(" · logout {pct}% ({}m left)", left / 1200 + 1));
        }
    }
    Some(line)
}

impl State {
    fn queue_config(&mut self) {
        for (k, v) in self.config.stored() {
            self.store.push((k.to_string(), v));
        }
    }

    /// A random u64 below `bound` (0 when the bound is 0).
    fn rand_below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545F4914F6CDD1D) % bound
    }

    /// A random count of ticks in the given seconds range (inclusive).
    fn rand_secs_ticks(&mut self, min_secs: f64, max_secs: f64) -> u64 {
        let lo = (min_secs * 20.0).round() as u64;
        let hi = (max_secs * 20.0).round() as u64;
        if hi <= lo {
            return lo.max(1);
        }
        lo + self.rand_below(hi - lo + 1)
    }

    /// The delay until the next timed action, in ticks.
    fn next_action_delay(&mut self) -> u64 {
        if self.config.random_interval {
            let (lo, hi) = (self.config.min_secs, self.config.max_secs);
            self.rand_secs_ticks(lo, hi)
        } else {
            ((self.config.action_delay * 20.0).round() as u64).max(1)
        }
    }

    /// Clear the run state (timers, phases, rotation), keeping the config.
    fn clear_run(&mut self) {
        self.afk_ticks = 0;
        self.logout_warned = false;
        self.action_due = 0;
        self.pause_remaining = 0;
        self.walk_phase = 0;
        self.walk_tick = 0;
        self.sneak_on = false;
        self.spin_vel = 0.0;
        self.spin_yaw = 0.0;
        self.mouse_anchor = None;
        self.mouse_target = (0.0, 0.0);
        self.mouse_timer = 0;
        self.jitter_offset = (0.0, 0.0);
        self.jitter_timer = 0;
        self.eating = false;
        self.eat_ticks = 0;
        self.eat_restore = 0;
        self.hotbar_due = 0;
        self.offhand_due = 0;
        self.offhand_hold_left = 0;
        self.inv_due = 0;
        self.inv_hold_left = 0;
        self.inv_open_by_us = false;
        self.chat_due = 0;
        self.cmd_due = 0;
        self.last_health = -1.0;
        self.keys = Keys::default();
        self.rotation = None;
    }
}

/// Toggle the mod on/off. Returns the new state. Turning it on starts
/// a fresh run: timers, the walk cycle, the spin and the mouse anchor
/// all restart from the player's current position and look.
pub fn toggle() -> bool {
    let mut s = state();
    s.active = !s.active;
    s.clear_run();
    if s.active {
        let action = s.next_action_delay();
        s.action_due = s.tick + action;
        let (lo, hi) = (s.config.hotbar_min, s.config.hotbar_max);
        let hotbar = s.rand_secs_ticks(lo, hi);
        s.hotbar_due = s.tick + hotbar;
        let (lo, hi) = (s.config.offhand_min, s.config.offhand_max);
        let offhand = s.rand_secs_ticks(lo, hi);
        s.offhand_due = s.tick + offhand;
        let (lo, hi) = (s.config.inv_min, s.config.inv_max);
        let inv = s.rand_secs_ticks(lo, hi);
        s.inv_due = s.tick + inv;
        let (lo, hi) = (s.config.chat_min, s.config.chat_max);
        let chat = s.rand_secs_ticks(lo, hi);
        s.chat_due = s.tick + chat;
        let (lo, hi) = (s.config.cmd_min, s.config.cmd_max);
        let cmd = s.rand_secs_ticks(lo, hi);
        s.cmd_due = s.tick + cmd;
    }
    s.active
}

/// Reset the run, e.g. on joining a server: the mod is off until it
/// is toggled on again.
pub fn reset() {
    let mut s = state();
    s.active = false;
    s.clear_run();
}

/// The WASD keys for the current walk-cycle phase.
fn walk_keys(phase: usize) -> Keys {
    match phase % 4 {
        0 => Keys {
            forward: true,
            ..Keys::default()
        },
        1 => Keys {
            right: true,
            ..Keys::default()
        },
        2 => Keys {
            backward: true,
            ..Keys::default()
        },
        _ => Keys {
            left: true,
            ..Keys::default()
        },
    }
}

/// One client tick.
pub fn tick(snap: Snapshot) -> TickOut {
    let mut out = TickOut::default();
    let mut s = state();
    s.tick += 1;
    if !s.active {
        s.keys = Keys::default();
        s.rotation = None;
        return out;
    }
    s.afk_ticks += 1;

    // Auto-logout: warn at 80% of the time, disconnect at 100%.
    if s.config.auto_logout && s.config.logout_minutes > 0.0 {
        let total = (s.config.logout_minutes * 1200.0) as u64;
        if total > 0 {
            if s.afk_ticks >= total {
                out.disconnect = Some("[LetMeAFK] Auto-logout time reached.".to_string());
                s.keys = Keys::default();
                s.rotation = None;
                return out;
            }
            if !s.logout_warned && s.afk_ticks >= total * 4 / 5 {
                s.logout_warned = true;
                let left = (total - s.afk_ticks) / 1200 + 1;
                out.lines.push(format!(
                    "[LetMeAFK] Auto-logout in about {left} minute(s) — toggle off to stay."
                ));
            }
        }
    }

    // Auto-disconnect: every 10 ticks, any non-ignored player within
    // the radius disconnects.
    if s.config.auto_disconnect && s.tick.is_multiple_of(DISCONNECT_CHECK_TICKS) {
        let radius = s.config.disconnect_radius;
        for (name, pos) in &snap.nearby {
            let dx = pos[0] - snap.player_pos[0];
            let dy = pos[1] - snap.player_pos[1];
            let dz = pos[2] - snap.player_pos[2];
            if dx * dx + dy * dy + dz * dz <= radius * radius && !s.config.is_ignored(name) {
                out.disconnect = Some(format!("[LetMeAFK] Player nearby — disconnected ({name})."));
                s.keys = Keys::default();
                s.rotation = None;
                return out;
            }
        }
    }

    // Auto-disconnect on damage: any health drop disconnects. No
    // screen gate — being hit while a menu is open is exactly when
    // you want out. A first read only records the value, and a rise
    // (healing, respawn) just updates it. The baseline is recorded
    // on every tick, toggle or not, so damage taken while the
    // feature is off never fires a disconnect when it is switched
    // back on.
    if snap.health > 0.0 {
        if s.config.disconnect_on_damage && s.last_health > 0.0 && snap.health < s.last_health {
            out.disconnect = Some("[LetMeAFK] Took damage — disconnected.".to_string());
            s.keys = Keys::default();
            s.rotation = None;
            return out;
        }
        s.last_health = snap.health;
    }

    // A screen being open stops only the movement keys and the
    // inventory-affecting features, as the original does: the timed
    // actions, eating, the camera and chat keep running behind a
    // menu, so the mod works in the background.
    let screen_open = snap.screen_open;

    // --- Random pause: the original rolls a 0.5% chance every tick
    // and then stands still for 20-59 ticks, freezing the walk cycle,
    // the timed actions and eating alike.
    if s.pause_remaining > 0 {
        s.pause_remaining -= 1;
    }
    if s.config.random_pause
        && s.pause_remaining == 0
        && s.rand_below(PAUSE_CHANCE_DENOMINATOR) == 0
    {
        s.pause_remaining = PAUSE_MIN_TICKS + s.rand_below(PAUSE_MAX_TICKS - PAUSE_MIN_TICKS + 1);
    }
    let paused = s.pause_remaining > 0;

    // --- Eating (takes over the hand until it finishes) ---
    // `eat_touched_slot` tracks whether this tick already chose a
    // hotbar slot for eating, so the random hotbar switch below does
    // not overwrite it (or the restore) in the same tick.
    let mut eat_touched_slot = false;
    if s.eating && !paused {
        s.eat_ticks += 1;
        let done = s.eat_ticks >= EAT_TICKS || (s.eat_ticks > 5 && !snap.using_item);
        if done {
            s.eating = false;
            out.hold_use = Some(false);
            if s.eat_restore != snap.selected_slot {
                out.select_slot = Some(s.eat_restore as u8);
                eat_touched_slot = true;
            }
        }
    } else if s.config.auto_eat
        && !paused
        && snap.food_level >= 0
        && snap.food_level <= s.config.eat_below as i32
        && !snap.food_slots.is_empty()
    {
        let slot = snap.food_slots[0];
        s.eating = true;
        s.eat_ticks = 0;
        s.eat_restore = snap.selected_slot;
        if slot != snap.selected_slot {
            out.select_slot = Some(slot as u8);
            eat_touched_slot = true;
        }
        out.hold_use = Some(true);
    }

    // --- Timed actions: jump / swing / sneak toggle / punch / use ---
    // A screen or an in-progress eat does not stop jump, swing or
    // sneak: the original does those behind menus too. Punch and use
    // are clicks, so the game itself drops them while a screen is
    // open (see the adapter), and they wait while eating, so a click
    // never cancels a bite.
    if !paused && s.tick >= s.action_due {
        if s.config.jump && snap.on_ground {
            out.keys.jump = true;
        }
        if s.config.swing {
            out.swing = true;
        }
        if s.config.punch && !s.eating {
            out.punch = true;
        }
        if s.config.use_item && !s.eating {
            out.use_item = true;
        }
        if s.config.sneak {
            s.sneak_on = !s.sneak_on;
        }
        let delay = s.next_action_delay();
        s.action_due = s.tick + delay;
    }

    // --- Random hotbar switch ---
    if !s.eating
        && !eat_touched_slot
        && !screen_open
        && s.config.random_hotbar
        && s.tick >= s.hotbar_due
    {
        let mut slot = s.rand_below(9) as usize;
        if slot == snap.selected_slot {
            slot = (slot + 1) % 9;
        }
        out.select_slot = Some(slot as u8);
        let (lo, hi) = (s.config.hotbar_min, s.config.hotbar_max);
        let delay = s.rand_secs_ticks(lo, hi);
        s.hotbar_due = s.tick + delay;
    }

    // --- Offhand swap (swap, hold, swap back) ---
    if !s.eating && !screen_open && s.config.offhand_swap {
        if s.offhand_hold_left > 0 {
            s.offhand_hold_left -= 1;
            if s.offhand_hold_left == 0 {
                out.swap_offhand = true;
                let (lo, hi) = (s.config.offhand_min, s.config.offhand_max);
                let delay = s.rand_secs_ticks(lo, hi);
                s.offhand_due = s.tick + delay;
            }
        } else if s.tick >= s.offhand_due {
            out.swap_offhand = true;
            s.offhand_hold_left = (s.config.offhand_hold * 20.0).round() as u64;
        }
    }

    // --- Open the inventory for a while, then close it ---
    if !s.eating && s.config.open_inventory {
        if s.inv_hold_left > 0 {
            s.inv_hold_left -= 1;
            if !screen_open {
                // The player closed it themselves: stop waiting.
                s.inv_hold_left = 0;
                s.inv_open_by_us = false;
            } else if s.inv_hold_left == 0 && s.inv_open_by_us {
                out.close_screen = true;
                s.inv_open_by_us = false;
                let (lo, hi) = (s.config.inv_min, s.config.inv_max);
                let delay = s.rand_secs_ticks(lo, hi);
                s.inv_due = s.tick + delay;
            }
        } else if !screen_open && s.tick >= s.inv_due {
            out.open_inventory = true;
            s.inv_open_by_us = true;
            s.inv_hold_left = (s.config.inv_hold * 20.0).round() as u64;
        }
    }

    // --- Auto chat ---
    if s.config.auto_chat && !s.config.chat_messages.is_empty() && s.tick >= s.chat_due {
        let count = s.config.chat_messages.len() as u64;
        let idx = s.rand_below(count) as usize;
        out.send_chat = Some(s.config.chat_messages[idx].clone());
        let (lo, hi) = (s.config.chat_min, s.config.chat_max);
        let delay = s.rand_secs_ticks(lo, hi);
        s.chat_due = s.tick + delay;
    }

    // --- Auto command: a small command at random, so the server
    // sees activity without a public chat message. The default list
    // is commands that do not exist, so the server just answers
    // "Unknown command" to the player only. It never takes a chat
    // message's turn: when both fire on one tick, the chat goes out
    // and the command waits for its next slot.
    if s.config.auto_command && !s.config.commands.is_empty() && s.tick >= s.cmd_due {
        let count = s.config.commands.len() as u64;
        let idx = s.rand_below(count) as usize;
        let mut cmd = s.config.commands[idx].clone();
        if !cmd.starts_with('/') {
            cmd.insert(0, '/');
        }
        if out.send_chat.is_none() {
            out.send_chat = Some(cmd);
        }
        let (lo, hi) = (s.config.cmd_min, s.config.cmd_max);
        let delay = s.rand_secs_ticks(lo, hi);
        s.cmd_due = s.tick + delay;
    }

    // --- Movement keys (WASD cycle) ---
    // 40 ticks walking, 10 ticks standing, per phase. The cycle
    // keeps advancing behind a screen, as the original's does — only
    // the keys are held back, so it resumes mid-cycle instead of
    // restarting, and never walks the player while a menu is open.
    let mut keys = Keys::default();
    if s.config.movement && !paused {
        if !screen_open && s.walk_tick < WALK_TICKS {
            keys = walk_keys(s.walk_phase);
        }
        s.walk_tick += 1;
        if s.walk_tick >= WALK_TICKS + WALK_PAUSE_TICKS {
            s.walk_tick = 0;
            s.walk_phase = (s.walk_phase + 1) % 4;
        }
    }
    if s.config.sneak {
        keys.sneak = s.sneak_on;
    }
    if out.keys.jump {
        keys.jump = true;
    }
    s.keys = keys;
    out.keys = keys;

    // --- Rotation: spin, mouse movement, jitter ---
    // Behind a screen too (the original turns the camera there);
    // only a random pause holds the view still.
    if !paused {
        out.rotation = compute_rotation(&mut s, &snap);
    }
    s.rotation = out.rotation;

    out
}

/// This tick's look, from the spin / mouse-movement / jitter settings.
/// Spin wins over mouse movement; jitter nudges whatever is running,
// or the player's own look when nothing else is.
fn compute_rotation(s: &mut State, snap: &Snapshot) -> Option<(f32, f32)> {
    // Re-roll the jitter offset every few ticks.
    if s.jitter_timer == 0 {
        if s.config.mouse_jitter {
            let yaw = (s.rand_below(2000) as f32 / 1000.0 - 1.0) * JITTER_YAW_STRENGTH;
            let pitch = (s.rand_below(2000) as f32 / 1000.0 - 1.0) * JITTER_PITCH_STRENGTH;
            s.jitter_offset = (yaw, pitch);
        } else {
            s.jitter_offset = (0.0, 0.0);
        }
        s.jitter_timer = JITTER_MIN_TICKS + s.rand_below(JITTER_MAX_TICKS - JITTER_MIN_TICKS + 1);
    } else {
        s.jitter_timer -= 1;
    }

    let mut target: Option<(f32, f32)> = None;
    if s.config.spin {
        // Smooth spin: the yaw velocity lerps toward the target speed
        // (degrees per tick), and the pitch rides a sine wave.
        if s.spin_vel == 0.0 && s.spin_yaw == 0.0 {
            s.spin_yaw = snap.yaw;
        }
        let want = (360.0 / (s.config.spin_speed * 20.0)) as f32;
        // The original lerps the yaw velocity at 0.02 a tick.
        s.spin_vel += (want - s.spin_vel) * 0.02;
        s.spin_yaw += s.spin_vel;
        let pitch = SPIN_PITCH_WAVE * ((s.afk_ticks as f32 * 0.03).sin());
        target = Some((s.spin_yaw, pitch));
    } else {
        s.spin_vel = 0.0;
        s.spin_yaw = 0.0;
        if s.config.mouse_movement {
            if s.mouse_anchor.is_none() {
                s.mouse_anchor = Some((snap.yaw, snap.pitch));
                s.mouse_target = (snap.yaw, snap.pitch);
                s.mouse_timer = MOUSE_RETARGET_TICKS;
            }
            if s.mouse_timer == 0 {
                let (ay, ap) = s.mouse_anchor.unwrap();
                let range = s.config.mouse_range as f32;
                let dyaw = (s.rand_below(2000) as f32 / 1000.0 - 1.0) * range;
                let dpitch = (s.rand_below(2000) as f32 / 1000.0 - 1.0) * range / 2.0;
                s.mouse_target = (ay + dyaw, (ap + dpitch).clamp(-90.0, 90.0));
                s.mouse_timer = MOUSE_RETARGET_TICKS;
            } else {
                s.mouse_timer -= 1;
            }
            // Lerp the current look toward the target, at the
            // original's 0.03 a tick — slow and smooth, not a snap.
            let yaw = snap.yaw + (s.mouse_target.0 - snap.yaw) * 0.03;
            let pitch = snap.pitch + (s.mouse_target.1 - snap.pitch) * 0.03;
            target = Some((yaw, pitch));
        }
    }

    match target {
        Some((yaw, pitch)) => Some((
            yaw + s.jitter_offset.0,
            (pitch + s.jitter_offset.1).clamp(-90.0, 90.0),
        )),
        None if s.config.mouse_jitter => Some((
            snap.yaw + s.jitter_offset.0,
            (snap.pitch + s.jitter_offset.1).clamp(-90.0, 90.0),
        )),
        None => None,
    }
}

/// What a manual command does: any chat lines for the player.
#[derive(Default)]
pub struct CommandOut {
    pub lines: Vec<String>,
}

/// The usages `register_client_commands` declares, one per form
/// `command()` answers. Kept here, next to the parser, so a test can
/// check the chat tree stays in sync with what the mod actually runs.
/// Argument kinds are the ones Starling 0.9.0's `Command` offers
/// (`integer`, `word`); `Command::new` makes each level 0, the only
/// level a client command may have.
pub const COMMAND_USAGES: &[&str] = &[
    "lmafk",
    "lmafk status",
    "lmafk help",
    "lmafk toggle",
    "lmafk reset",
    "lmafk interval set <secs:number>",
    "lmafk interval random <min:number> <max:number>",
    "lmafk interval",
    "lmafk swing",
    "lmafk punch",
    "lmafk use",
    "lmafk jump",
    "lmafk spin",
    "lmafk spin <secs:number>",
    "lmafk sneak",
    "lmafk mousemovement",
    "lmafk mousemovement <range:number>",
    "lmafk movement",
    "lmafk eat",
    "lmafk eat <level:integer>",
    "lmafk hotbar",
    "lmafk offhand",
    "lmafk inventory",
    "lmafk chat",
    "lmafk chat add <message:text>",
    "lmafk chat list",
    "lmafk chat clear",
    "lmafk command",
    "lmafk cmd add <command:text>",
    "lmafk cmd remove <index:integer>",
    "lmafk cmd list",
    "lmafk cmd clear",
    "lmafk command min <secs:number>",
    "lmafk command max <secs:number>",
    "lmafk damage",
    "lmafk disconnect",
    "lmafk disconnect <radius:number>",
    "lmafk ignore add <name:word>",
    "lmafk ignore remove <name:word>",
    "lmafk ignore list",
    "lmafk ignore",
    "lmafk logout <minutes:integer>",
    "lmafk logout off",
    "lmafk hud",
    "lmafk jitter",
    "lmafk pause",
    "lmafk random",
    "lmafk random <min:number> <max:number>",
];

const HELP: &[&str] = &[
    "/lmafk - turn LetMeAFK on/off",
    "/lmafk status - show what is on and how long you have been AFK",
    "/lmafk reset - restore the original AntiAfk defaults",
    "/lmafk interval set <secs> - seconds between timed actions (default 5)",
    "/lmafk interval random <min> <max> - random interval between min/max seconds",
    "/lmafk swing / jump / spin / sneak / movement - toggle each action",
    "/lmafk punch - attack whatever is under the crosshair; /lmafk use - use the held item",
    "/lmafk damage - disconnect when you take damage",
    "/lmafk spin <secs> - seconds per full 360-degree spin",
    "/lmafk mousemovement - toggle random camera moves",
    "/lmafk eat - toggle auto-eat; /lmafk eat <level> - eat at/below this hunger (1-20)",
    "/lmafk hotbar / offhand / inventory - toggle hotbar shuffle, offhand swap, inventory opens",
    "/lmafk chat add <message> / list / clear - auto chat messages",
    "/lmafk command - send a small command now and then instead of chat; /lmafk cmd add <cmd> / remove <n> / list / clear - the command list; /lmafk command min <secs> / max <secs> - how often",
    "/lmafk disconnect - toggle auto-disconnect; /lmafk disconnect <blocks> - radius",
    "/lmafk ignore add <name> / remove <name> / list - players auto-disconnect ignores",
    "/lmafk logout <minutes> / off - auto-logout after N minutes (0 = off)",
    "/lmafk hud / jitter / pause / random - toggle the HUD, mouse jitter, random pauses, random intervals",
];

fn on_off(b: bool) -> &'static str {
    if b { "On" } else { "Off" }
}

/// A command the player typed, without its slash. None when it is not
/// one of this mod's commands.
pub fn command(line: &str) -> Option<CommandOut> {
    let line = line.strip_prefix('/').unwrap_or(line);
    let mut words = line.split_whitespace();
    if words.next()? != "lmafk" {
        return None;
    }
    let mut out = CommandOut::default();
    let mut s = state();
    let args: Vec<&str> = words.collect();
    match args.as_slice() {
        ["status"] => {
            let c = &s.config;
            out.lines.push(format!(
                "[LetMeAFK] {} - jump {}, sneak {}, swing {}, punch {}, use {}, movement {}, spin {} ({}s/rev), mouse {}, jitter {}, pause {}, random interval {} ({}-{}s), eat {} (below {}), hotbar {}, offhand {}, inventory {}, chat {} ({} msgs), cmd {} ({} cmds), disconnect {} ({} blocks, {} ignored), damage {}, logout {}, HUD {}",
                on_off(s.active),
                on_off(c.jump),
                on_off(c.sneak),
                on_off(c.swing),
                on_off(c.punch),
                on_off(c.use_item),
                on_off(c.movement),
                on_off(c.spin),
                c.spin_speed,
                on_off(c.mouse_movement),
                on_off(c.mouse_jitter),
                on_off(c.random_pause),
                on_off(c.random_interval),
                c.min_secs,
                c.max_secs,
                on_off(c.auto_eat),
                c.eat_below,
                on_off(c.random_hotbar),
                on_off(c.offhand_swap),
                on_off(c.open_inventory),
                on_off(c.auto_chat),
                c.chat_messages.len(),
                on_off(c.auto_command),
                c.commands.len(),
                on_off(c.auto_disconnect),
                c.disconnect_radius,
                c.ignored_players.len(),
                on_off(c.disconnect_on_damage),
                if c.auto_logout && c.logout_minutes > 0.0 {
                    format!("{} min", c.logout_minutes)
                } else {
                    "Off".to_string()
                },
                on_off(c.hud),
            ));
        }
        ["help"] => out.lines.extend(HELP.iter().map(|l| l.to_string())),
        ["reset"] => {
            s.config = Config::default();
            s.queue_config();
            out.lines.push(
                "[LetMeAFK] Settings reset to the original AntiAfk defaults: jump and auto-eat on, everything else off."
                    .to_string(),
            );
        }
        [] | ["toggle"] => {
            // Inline `toggle()` here: `s` already holds the state
            // lock, and `toggle()` would lock it again.
            s.active = !s.active;
            s.clear_run();
            if s.active {
                let delay = s.next_action_delay();
                s.action_due = s.tick + delay;
                let (lo, hi) = (s.config.hotbar_min, s.config.hotbar_max);
                let delay = s.rand_secs_ticks(lo, hi);
        s.hotbar_due = s.tick + delay;
                let (lo, hi) = (s.config.offhand_min, s.config.offhand_max);
                let delay = s.rand_secs_ticks(lo, hi);
                s.offhand_due = s.tick + delay;
                let (lo, hi) = (s.config.inv_min, s.config.inv_max);
                let delay = s.rand_secs_ticks(lo, hi);
                s.inv_due = s.tick + delay;
                let (lo, hi) = (s.config.chat_min, s.config.chat_max);
                let delay = s.rand_secs_ticks(lo, hi);
        s.chat_due = s.tick + delay;
                let (lo, hi) = (s.config.cmd_min, s.config.cmd_max);
                let delay = s.rand_secs_ticks(lo, hi);
        s.cmd_due = s.tick + delay;
            }
            out.lines.push(format!(
                "[LetMeAFK] LetMeAFK {}.",
                on_off(s.active)
            ));
        }
        ["interval", "set", n] => match n.parse::<f64>() {
            Ok(v) if (0.5..=60.0).contains(&v) => {
                s.config.action_delay = v;
                s.config.random_interval = false;
                s.queue_config();
                out.lines.push(format!(
                    "[LetMeAFK] Timed actions every {v} seconds (fixed interval)."
                ));
            }
            _ => out.lines.push(
                "[LetMeAFK] Interval is 0.5-60 seconds, e.g. /lmafk interval set 3.".to_string(),
            ),
        },
        ["interval", "random", min, max] => {
            match (min.parse::<f64>(), max.parse::<f64>()) {
                (Ok(lo), Ok(hi)) if (0.5..=60.0).contains(&lo) && (0.5..=60.0).contains(&hi) && lo <= hi => {
                    s.config.min_secs = lo;
                    s.config.max_secs = hi;
                    s.config.random_interval = true;
                    s.queue_config();
                    out.lines.push(format!(
                        "[LetMeAFK] Random interval on: {lo}-{hi} seconds between timed actions."
                    ));
                }
                _ => out.lines.push(
                    "[LetMeAFK] Random interval is /lmafk interval random <min> <max> seconds, e.g. 1 7."
                        .to_string(),
                ),
            }
        }
        ["interval"] => {
            if s.config.random_interval {
                out.lines.push(format!(
                    "[LetMeAFK] Random interval: {}-{} seconds.",
                    s.config.min_secs, s.config.max_secs
                ));
            } else {
                out.lines.push(format!(
                    "[LetMeAFK] Fixed interval: {} seconds.",
                    s.config.action_delay
                ));
            }
        }
        ["random", min, max] => {
            match (min.parse::<f64>(), max.parse::<f64>()) {
                (Ok(lo), Ok(hi)) if (0.5..=60.0).contains(&lo) && (0.5..=60.0).contains(&hi) && lo <= hi => {
                    s.config.min_secs = lo;
                    s.config.max_secs = hi;
                    s.config.random_interval = true;
                    s.queue_config();
                    out.lines.push(format!(
                        "[LetMeAFK] Random interval on: {lo}-{hi} seconds between timed actions."
                    ));
                }
                _ => out.lines.push(
                    "[LetMeAFK] Random interval is /lmafk random <min> <max> seconds, e.g. 1 7."
                        .to_string(),
                ),
            }
        }
        ["swing"] => {
            s.config.swing = !s.config.swing;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Swing {}.",
                on_off(s.config.swing)
            ));
        }
        ["punch"] => {
            s.config.punch = !s.config.punch;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Punch {}.",
                on_off(s.config.punch)
            ));
        }
        ["use"] => {
            s.config.use_item = !s.config.use_item;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Use item {}.",
                on_off(s.config.use_item)
            ));
        }
        ["jump"] => {
            s.config.jump = !s.config.jump;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Auto jump {}.",
                on_off(s.config.jump)
            ));
        }
        ["sneak"] => {
            s.config.sneak = !s.config.sneak;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Sneak {}.",
                on_off(s.config.sneak)
            ));
        }
        ["movement"] => {
            s.config.movement = !s.config.movement;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Player movement {}.",
                on_off(s.config.movement)
            ));
        }
        ["spin"] => {
            s.config.spin = !s.config.spin;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Auto spin {}.",
                on_off(s.config.spin)
            ));
        }
        ["spin", n] => match n.parse::<f64>() {
            Ok(v) if (2.0..=30.0).contains(&v) => {
                s.config.spin_speed = v;
                s.queue_config();
                out.lines.push(format!(
                    "[LetMeAFK] Spin speed: one full turn every {v} seconds."
                ));
            }
            _ => out.lines.push(
                "[LetMeAFK] Spin speed is 2-30 seconds per full turn.".to_string(),
            ),
        },
        ["mousemovement"] => {
            s.config.mouse_movement = !s.config.mouse_movement;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Mouse movement {}.",
                on_off(s.config.mouse_movement)
            ));
        }
        ["mousemovement", n] => match n.parse::<f64>() {
            Ok(v) if (5.0..=90.0).contains(&v) => {
                s.config.mouse_range = v;
                s.queue_config();
                out.lines.push(format!(
                    "[LetMeAFK] Mouse movement range: {v} degrees around the anchor."
                ));
            }
            _ => out.lines.push(
                "[LetMeAFK] Mouse movement range is 5-90 degrees.".to_string(),
            ),
        },
        ["jitter"] => {
            s.config.mouse_jitter = !s.config.mouse_jitter;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Mouse jitter {}.",
                on_off(s.config.mouse_jitter)
            ));
        }
        ["pause"] => {
            s.config.random_pause = !s.config.random_pause;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Random pauses {}.",
                on_off(s.config.random_pause)
            ));
        }
        ["random"] => {
            s.config.random_interval = !s.config.random_interval;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Random intervals {}.",
                on_off(s.config.random_interval)
            ));
        }
        ["eat"] => {
            s.config.auto_eat = !s.config.auto_eat;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Auto eat {}.",
                on_off(s.config.auto_eat)
            ));
        }
        ["eat", n] => match n.parse::<u8>() {
            Ok(v) if (1..=20).contains(&v) => {
                s.config.eat_below = v;
                s.queue_config();
                out.lines.push(format!(
                    "[LetMeAFK] Auto eat at hunger {v} or below."
                ));
            }
            _ => out.lines.push(
                "[LetMeAFK] Eat level is 1-20 (hunger out of 20).".to_string(),
            ),
        },
        ["hotbar"] => {
            s.config.random_hotbar = !s.config.random_hotbar;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Random hotbar {}.",
                on_off(s.config.random_hotbar)
            ));
        }
        ["offhand"] => {
            s.config.offhand_swap = !s.config.offhand_swap;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Offhand swap {}.",
                on_off(s.config.offhand_swap)
            ));
        }
        ["inventory"] => {
            s.config.open_inventory = !s.config.open_inventory;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Open inventory {}.",
                on_off(s.config.open_inventory)
            ));
        }
        ["chat"] => {
            s.config.auto_chat = !s.config.auto_chat;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Auto chat {}.",
                on_off(s.config.auto_chat)
            ));
        }
        ["chat", "add", rest @ ..] => {
            let msg = rest.join(" ");
            if msg.is_empty() {
                out.lines.push(
                    "[LetMeAFK] Give a message, e.g. /lmafk chat add Still here!".to_string(),
                );
            } else if s.config.chat_messages.iter().any(|m| m == &msg) {
                out.lines.push(
                    "[LetMeAFK] That message is already on the list.".to_string(),
                );
            } else {
                s.config.chat_messages.push(msg.clone());
                s.queue_config();
                out.lines.push(format!(
                    "[LetMeAFK] Added chat message: {msg} ({} total).",
                    s.config.chat_messages.len()
                ));
            }
        }
        ["chat", "list"] => {
            if s.config.chat_messages.is_empty() {
                out.lines.push(
                    "[LetMeAFK] No chat messages. Add one with /lmafk chat add <message>."
                        .to_string(),
                );
            } else {
                out.lines.push(format!(
                    "[LetMeAFK] Chat messages ({}): {}.",
                    s.config.chat_messages.len(),
                    s.config.chat_messages.join(" | ")
                ));
            }
        }
        ["chat", "clear"] => {
            s.config.chat_messages.clear();
            s.queue_config();
            out.lines.push("[LetMeAFK] Chat messages cleared.".to_string());
        }
        ["command"] => {
            s.config.auto_command = !s.config.auto_command;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Auto command {}.",
                on_off(s.config.auto_command)
            ));
        }
        ["cmd", "add", rest @ ..] => {
            let mut cmd = rest.join(" ");
            if cmd.is_empty() {
                out.lines.push(
                    "[LetMeAFK] Give a command, e.g. /lmafk cmd add /zzz.".to_string(),
                );
            } else {
                if !cmd.starts_with('/') {
                    cmd.insert(0, '/');
                }
                if s.config.commands.iter().any(|c| c == &cmd) {
                    out.lines.push(
                        "[LetMeAFK] That command is already on the list.".to_string(),
                    );
                } else {
                    s.config.commands.push(cmd.clone());
                    s.queue_config();
                    out.lines.push(format!(
                        "[LetMeAFK] Added command: {cmd} ({} total).",
                        s.config.commands.len()
                    ));
                }
            }
        }
        ["cmd", "remove", n] => match n.parse::<usize>() {
            Ok(i) if (1..=s.config.commands.len()).contains(&i) => {
                let removed = s.config.commands.remove(i - 1);
                s.queue_config();
                out.lines.push(format!(
                    "[LetMeAFK] Removed command: {removed}."
                ));
            }
            _ => out.lines.push(
                "[LetMeAFK] Remove is /lmafk cmd remove <number> — see /lmafk cmd list."
                    .to_string(),
            ),
        },
        ["cmd", "list"] => {
            if s.config.commands.is_empty() {
                out.lines.push(
                    "[LetMeAFK] No commands. Add one with /lmafk cmd add <command>."
                        .to_string(),
                );
            } else {
                let numbered: Vec<String> = s
                    .config
                    .commands
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}: {c}", i + 1))
                    .collect();
                out.lines.push(format!(
                    "[LetMeAFK] Commands ({}): {}.",
                    s.config.commands.len(),
                    numbered.join(" | ")
                ));
            }
        }
        ["cmd", "clear"] => {
            s.config.commands.clear();
            s.queue_config();
            out.lines.push("[LetMeAFK] Commands cleared.".to_string());
        }
        ["command", "min", n] => match n.parse::<f64>() {
            Ok(v) if (1.0..=1200.0).contains(&v) => {
                s.config.cmd_min = v;
                if s.config.cmd_max < v {
                    s.config.cmd_max = v;
                }
                s.queue_config();
                out.lines.push(format!(
                    "[LetMeAFK] Auto-command at least every {v} seconds."
                ));
            }
            _ => out.lines.push(
                "[LetMeAFK] Command min is 1-1200 seconds.".to_string(),
            ),
        },
        ["command", "max", n] => match n.parse::<f64>() {
            Ok(v) if (1.0..=3600.0).contains(&v) => {
                s.config.cmd_max = v;
                if s.config.cmd_min > v {
                    s.config.cmd_min = v;
                }
                s.queue_config();
                out.lines.push(format!(
                    "[LetMeAFK] Auto-command at most every {v} seconds."
                ));
            }
            _ => out.lines.push(
                "[LetMeAFK] Command max is 1-3600 seconds.".to_string(),
            ),
        }
        ["damage"] => {
            s.config.disconnect_on_damage = !s.config.disconnect_on_damage;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Disconnect on damage {}.",
                on_off(s.config.disconnect_on_damage)
            ));
        }
        ["disconnect"] => {
            s.config.auto_disconnect = !s.config.auto_disconnect;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] Auto disconnect {}.",
                on_off(s.config.auto_disconnect)
            ));
        }
        ["disconnect", n] => match n.parse::<f64>() {
            Ok(v) if (1.0..=64.0).contains(&v) => {
                s.config.disconnect_radius = v;
                s.queue_config();
                out.lines.push(format!(
                    "[LetMeAFK] Auto disconnect radius: {v} blocks."
                ));
            }
            _ => out.lines.push(
                "[LetMeAFK] Disconnect radius is 1-64 blocks.".to_string(),
            ),
        },
        ["ignore", "add", name] => {
            if s.config.is_ignored(name) {
                out.lines.push(format!(
                    "[LetMeAFK] {name} is already ignored."
                ));
            } else {
                s.config.ignored_players.push((*name).to_string());
                s.queue_config();
                out.lines.push(format!(
                    "[LetMeAFK] Ignoring {name} for auto-disconnect."
                ));
            }
        }
        ["ignore", "remove", name] => {
            let before = s.config.ignored_players.len();
            s.config
                .ignored_players
                .retain(|n| !n.eq_ignore_ascii_case(name));
            if s.config.ignored_players.len() == before {
                out.lines.push(format!(
                    "[LetMeAFK] {name} is not on the ignore list."
                ));
            } else {
                s.queue_config();
                out.lines.push(format!(
                    "[LetMeAFK] No longer ignoring {name}."
                ));
            }
        }
        ["ignore", "list"] | ["ignore"] => {
            if s.config.ignored_players.is_empty() {
                out.lines.push(
                    "[LetMeAFK] Ignore list is empty. Add with /lmafk ignore add <name>."
                        .to_string(),
                );
            } else {
                out.lines.push(format!(
                    "[LetMeAFK] Ignored players ({}): {}.",
                    s.config.ignored_players.len(),
                    s.config.ignored_players.join(", ")
                ));
            }
        }
        ["logout", "off"] => {
            s.config.auto_logout = false;
            s.config.logout_minutes = 0.0;
            s.queue_config();
            out.lines.push("[LetMeAFK] Auto-logout off.".to_string());
        }
        ["logout", n] => match n.parse::<f64>() {
            Ok(v) if (0.0..=240.0).contains(&v) => {
                s.config.logout_minutes = v;
                s.config.auto_logout = v > 0.0;
                s.queue_config();
                if v > 0.0 {
                    out.lines.push(format!(
                        "[LetMeAFK] Auto-logout after {v} minutes AFK."
                    ));
                } else {
                    out.lines.push("[LetMeAFK] Auto-logout off.".to_string());
                }
            }
            _ => out.lines.push(
                "[LetMeAFK] Logout time is 0-240 minutes (0 = off).".to_string(),
            ),
        },
        ["hud"] => {
            s.config.hud = !s.config.hud;
            s.queue_config();
            out.lines.push(format!(
                "[LetMeAFK] AFK timer HUD {}.",
                on_off(s.config.hud)
            ));
        }
        _ => out.lines.extend(HELP.iter().map(|l| l.to_string())),
    }
    Some(out)
}

/// The settings page's buttons, in order: the same clusters as
/// the game's settings page — actions, camera, timing, eating,
/// items, chat, safety, display — so each range sits under the
/// feature it belongs to instead of all ranges trailing at the end.
pub fn settings_buttons() -> Vec<String> {
    let c = &state().config;
    vec![
        format!("Auto jump: {}", on_off(c.jump)),
        format!("Sneak mode: {}", on_off(c.sneak)),
        format!("Swing hand: {}", on_off(c.swing)),
        format!("Punch: {}", on_off(c.punch)),
        format!("Use item: {}", on_off(c.use_item)),
        format!("Player movement: {}", on_off(c.movement)),
        format!("Auto spin: {}", on_off(c.spin)),
        format!("Spin speed: {}s per turn", c.spin_speed),
        format!("Mouse movement: {}", on_off(c.mouse_movement)),
        format!("Mouse range: {}°", c.mouse_range),
        format!("Mouse jitter: {}", on_off(c.mouse_jitter)),
        format!("Random pauses: {}", on_off(c.random_pause)),
        format!("Random intervals: {}", on_off(c.random_interval)),
        format!("Action delay: {}s", c.action_delay),
        format!("Random min: {}s", c.min_secs),
        format!("Random max: {}s", c.max_secs),
        format!("Auto eat: {}", on_off(c.auto_eat)),
        format!("Eat below hunger: {}", c.eat_below),
        format!("Random hotbar: {}", on_off(c.random_hotbar)),
        format!("Hotbar min: {}s", c.hotbar_min),
        format!("Hotbar max: {}s", c.hotbar_max),
        format!("Offhand swap: {}", on_off(c.offhand_swap)),
        format!("Offhand min: {}s", c.offhand_min),
        format!("Offhand max: {}s", c.offhand_max),
        format!("Offhand hold: {}s", c.offhand_hold),
        format!("Open inventory: {}", on_off(c.open_inventory)),
        format!("Inventory min: {}s", c.inv_min),
        format!("Inventory max: {}s", c.inv_max),
        format!("Inventory hold: {}s", c.inv_hold),
        format!("Auto chat: {}", on_off(c.auto_chat)),
        format!("Chat min: {}s", c.chat_min),
        format!("Chat max: {}s", c.chat_max),
        format!("Auto command: {}", on_off(c.auto_command)),
        format!("Command min: {}s", c.cmd_min),
        format!("Command max: {}s", c.cmd_max),
        format!("Auto disconnect: {}", on_off(c.auto_disconnect)),
        format!("Disconnect radius: {} blocks", c.disconnect_radius),
        format!("Disconnect on damage: {}", on_off(c.disconnect_on_damage)),
        format!("Auto logout: {}", on_off(c.auto_logout)),
        format!("Auto-logout after: {} min", c.logout_minutes),
        format!("AFK timer HUD: {}", on_off(c.hud)),
    ]
}

/// Cycle a range setting to its next choice.
fn cycle(choices: &[f64], cur: f64) -> f64 {
    choices
        .iter()
        .find(|&&v| v > cur)
        .copied()
        .unwrap_or(choices[0])
}

/// A click on the settings page: the button's setting moves to its next value.
pub fn settings_click(button: usize) {
    let mut s = state();
    match button {
        0 => s.config.jump = !s.config.jump,
        1 => s.config.sneak = !s.config.sneak,
        2 => s.config.swing = !s.config.swing,
        3 => s.config.punch = !s.config.punch,
        4 => s.config.use_item = !s.config.use_item,
        5 => s.config.movement = !s.config.movement,
        6 => s.config.spin = !s.config.spin,
        7 => s.config.spin_speed = cycle(&SPIN_SPEED_CHOICES, s.config.spin_speed),
        8 => s.config.mouse_movement = !s.config.mouse_movement,
        9 => s.config.mouse_range = cycle(&MOUSE_RANGE_CHOICES, s.config.mouse_range),
        10 => s.config.mouse_jitter = !s.config.mouse_jitter,
        11 => s.config.random_pause = !s.config.random_pause,
        12 => s.config.random_interval = !s.config.random_interval,
        13 => s.config.action_delay = cycle(&ACTION_DELAY_CHOICES, s.config.action_delay),
        14 => s.config.min_secs = cycle(&MIN_SECS_CHOICES, s.config.min_secs),
        15 => s.config.max_secs = cycle(&MAX_SECS_CHOICES, s.config.max_secs),
        16 => s.config.auto_eat = !s.config.auto_eat,
        17 => {
            let cur = s.config.eat_below as f64;
            s.config.eat_below = cycle(&EAT_BELOW_CHOICES, cur) as u8;
        }
        18 => s.config.random_hotbar = !s.config.random_hotbar,
        19 => s.config.hotbar_min = cycle(&HOTBAR_MIN_CHOICES, s.config.hotbar_min),
        20 => s.config.hotbar_max = cycle(&HOTBAR_MAX_CHOICES, s.config.hotbar_max),
        21 => s.config.offhand_swap = !s.config.offhand_swap,
        22 => s.config.offhand_min = cycle(&OFFHAND_MIN_CHOICES, s.config.offhand_min),
        23 => s.config.offhand_max = cycle(&OFFHAND_MAX_CHOICES, s.config.offhand_max),
        24 => s.config.offhand_hold = cycle(&OFFHAND_HOLD_CHOICES, s.config.offhand_hold),
        25 => s.config.open_inventory = !s.config.open_inventory,
        26 => s.config.inv_min = cycle(&INV_MIN_CHOICES, s.config.inv_min),
        27 => s.config.inv_max = cycle(&INV_MAX_CHOICES, s.config.inv_max),
        28 => s.config.inv_hold = cycle(&INV_HOLD_CHOICES, s.config.inv_hold),
        29 => s.config.auto_chat = !s.config.auto_chat,
        30 => s.config.chat_min = cycle(&CHAT_MIN_CHOICES, s.config.chat_min),
        31 => s.config.chat_max = cycle(&CHAT_MAX_CHOICES, s.config.chat_max),
        32 => s.config.auto_command = !s.config.auto_command,
        33 => s.config.cmd_min = cycle(&CMD_MIN_CHOICES, s.config.cmd_min),
        34 => s.config.cmd_max = cycle(&CMD_MAX_CHOICES, s.config.cmd_max),
        35 => s.config.auto_disconnect = !s.config.auto_disconnect,
        36 => {
            s.config.disconnect_radius =
                cycle(&DISCONNECT_RADIUS_CHOICES, s.config.disconnect_radius);
        }
        37 => s.config.disconnect_on_damage = !s.config.disconnect_on_damage,
        38 => s.config.auto_logout = !s.config.auto_logout,
        39 => {
            s.config.logout_minutes = cycle(&LOGOUT_MINUTES_CHOICES, s.config.logout_minutes);
            s.config.auto_logout = s.config.logout_minutes > 0.0;
        }
        40 => s.config.hud = !s.config.hud,
        _ => return,
    }
    s.queue_config();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One lock for the whole test run: every test holds it from its
    /// `reset()` to the end of its body, so parallel tests cannot
    /// interleave on the shared `STATE` (the failures were all
    /// cross-test interference, not engine bugs — e.g. `toggle()`
    /// returning false because another test had already toggled on).
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Reset the shared state, holding the test lock for the rest of
    /// the calling test. Keep the guard alive for the whole test body.
    fn reset() -> MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = state();
        *s = State::default();
        guard
    }

    fn snap() -> Snapshot {
        Snapshot {
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            screen_open: false,
            food_level: 20,
            health: 20.0,
            selected_slot: 0,
            food_slots: Vec::new(),
            player_pos: [0.0, 64.0, 0.0],
            nearby: Vec::new(),
            using_item: false,
        }
    }

    /// Turn the mod on with a quiet config: no random intervals, no
    /// timed actions for a while, no random pauses.
    fn quiet_on() {
        state().config.random_interval = false;
        state().config.action_delay = 60.0;
        state().config.random_pause = false;
        state().config.movement = true;
        assert!(toggle());
    }

    fn wait_ticks(n: u64, snap: Snapshot) -> TickOut {
        let mut out = TickOut::default();
        for _ in 0..n {
            out = tick(snap.clone());
        }
        out
    }

    #[test]
    fn inactive_tick_does_nothing() {
        let _guard = reset();
        let out = tick(snap());
        assert_eq!(out.keys, Keys::default());
        assert_eq!(out.rotation, None);
        assert!(!out.swing);
        assert!(!state().active);
    }

    #[test]
    fn toggle_on_starts_a_run_and_off_stops_it() {
        let _guard = reset();
        assert!(toggle());
        assert!(state().active);
        assert!(!toggle());
        assert!(!state().active);
        // Off again: the keys are cleared.
        assert_eq!(tick(snap()).keys, Keys::default());
    }

    #[test]
    fn walk_cycle_is_forward_right_back_left_with_pauses() {
        let _guard = reset();
        quiet_on();
        // 40 ticks walking forward.
        for _ in 0..40 {
            let out = tick(snap());
            assert!(out.keys.forward, "expected forward, got {:?}", out.keys);
        }
        // 10 ticks standing still.
        for _ in 0..10 {
            assert_eq!(tick(snap()).keys, Keys::default());
        }
        // Then right.
        let out = tick(snap());
        assert!(out.keys.right, "expected right, got {:?}", out.keys);
        // Right for 40, pause 10, then back.
        wait_ticks(39, snap());
        for _ in 0..10 {
            assert_eq!(tick(snap()).keys, Keys::default());
        }
        assert!(tick(snap()).keys.backward);
    }

    #[test]
    fn screen_open_holds_movement_keys() {
        // A screen holds the movement keys (the player must not walk
        // while a menu is open), but nothing else about the run stops.
        let _guard = reset();
        quiet_on();
        let mut s = snap();
        s.screen_open = true;
        let out = tick(s);
        assert_eq!(out.keys, Keys::default());
        assert_eq!(out.rotation, None);
    }

    #[test]
    fn timed_actions_fire_with_a_screen_open() {
        let _guard = reset();
        state().config.random_interval = false;
        state().config.action_delay = 1.0;
        state().config.random_pause = false;
        state().config.jump = true;
        state().config.swing = true;
        assert!(toggle());
        let mut s = snap();
        s.screen_open = true;
        let mut jumped = false;
        let mut swung = false;
        for _ in 0..40 {
            let out = tick(s.clone());
            jumped |= out.keys.jump;
            swung |= out.swing;
        }
        assert!(jumped, "no jump behind a screen");
        assert!(swung, "no swing behind a screen");
    }

    #[test]
    fn rotation_continues_with_a_screen_open() {
        let _guard = reset();
        state().config.spin = true;
        state().config.mouse_jitter = false;
        quiet_on();
        let mut s = snap();
        s.screen_open = true;
        let mut yaws = Vec::new();
        for _ in 0..20 {
            let (yaw, _) = tick(s.clone())
                .rotation
                .expect("spin must turn the camera behind a screen");
            yaws.push(yaw);
        }
        for w in yaws.windows(2) {
            assert!(w[1] > w[0], "yaw did not grow: {yaws:?}");
        }
    }

    #[test]
    fn walk_cycle_advances_behind_a_screen() {
        let _guard = reset();
        quiet_on();
        // 50 ticks behind a screen: 40 walking (keys held), 10
        // standing, then the cycle moves on to the right phase.
        let mut s = snap();
        s.screen_open = true;
        for _ in 0..50 {
            assert_eq!(tick(s.clone()).keys, Keys::default());
        }
        s.screen_open = false;
        let out = tick(s);
        assert!(out.keys.right, "expected right, got {:?}", out.keys);
    }

    #[test]
    fn timed_jump_fires_on_the_interval_only_on_ground() {
        let _guard = reset();
        state().config.random_interval = false;
        state().config.action_delay = 1.0;
        state().config.random_pause = false;
        state().config.swing = false;
        assert!(toggle());
        for _ in 0..19 {
            assert!(!tick(snap()).keys.jump);
        }
        assert!(tick(snap()).keys.jump);
        assert!(!tick(snap()).keys.jump);
        // Airborne: no jump on the next fire.
        let mut air = snap();
        air.on_ground = false;
        wait_ticks(18, snap());
        assert!(!tick(air).keys.jump);
    }

    #[test]
    fn punch_and_use_fire_on_the_timed_action() {
        let _guard = reset();
        state().config.random_interval = false;
        state().config.action_delay = 1.0;
        state().config.random_pause = false;
        state().config.punch = true;
        state().config.use_item = true;
        assert!(toggle());
        let mut punched = false;
        let mut used = false;
        for _ in 0..40 {
            let out = tick(snap());
            punched |= out.punch;
            used |= out.use_item;
        }
        assert!(punched, "no punch on the timed action");
        assert!(used, "no use on the timed action");
    }

    #[test]
    fn punch_and_use_stay_off_when_disabled() {
        let _guard = reset();
        state().config.random_interval = false;
        state().config.action_delay = 1.0;
        assert!(toggle());
        for _ in 0..40 {
            let out = tick(snap());
            assert!(!out.punch);
            assert!(!out.use_item);
        }
    }

    #[test]
    fn sneak_flips_each_timed_action() {
        let _guard = reset();
        state().config.random_interval = false;
        state().config.action_delay = 1.0;
        state().config.random_pause = false;
        state().config.sneak = true;
        state().config.swing = false;
        state().config.jump = false;
        assert!(toggle());
        wait_ticks(19, snap());
        assert!(tick(snap()).keys.sneak);
        wait_ticks(19, snap());
        assert!(!tick(snap()).keys.sneak);
    }

    #[test]
    fn swing_fires_on_the_timed_action() {
        let _guard = reset();
        state().config.swing = true;
        state().config.random_interval = false;
        state().config.action_delay = 1.0;
        state().config.random_pause = false;
        state().config.jump = false;
        assert!(toggle());
        for _ in 0..19 {
            assert!(!tick(snap()).swing);
        }
        assert!(tick(snap()).swing);
        assert!(!tick(snap()).swing);
    }

    #[test]
    fn random_interval_stays_in_bounds_and_varies() {
        let _guard = reset();
        state().config.swing = true;
        state().config.random_interval = true;
        state().config.min_secs = 1.0;
        state().config.max_secs = 7.0;
        state().config.random_pause = false;
        state().config.jump = false;
        assert!(toggle());
        let mut fires = Vec::new();
        for i in 1..=1400u64 {
            if tick(snap()).swing {
                fires.push(i);
            }
        }
        assert!(fires.len() >= 8, "too few fires: {fires:?}");
        let mut gaps: Vec<u64> = fires.windows(2).map(|w| w[1] - w[0]).collect();
        gaps.sort();
        assert!(
            gaps.iter().all(|&g| (20..=140).contains(&g)),
            "gaps out of bounds: {gaps:?}"
        );
        assert!(
            gaps.iter().any(|&g| g != gaps[0]),
            "interval never varied: {gaps:?}"
        );
    }

    #[test]
    fn random_pause_stretches_the_gap_between_actions() {
        let _guard = reset();
        state().config.swing = true;
        state().config.random_interval = false;
        state().config.action_delay = 0.5;
        state().config.random_pause = true;
        state().config.jump = false;
        assert!(toggle());
        let mut fires = Vec::new();
        for i in 1..=600u64 {
            if tick(snap()).swing {
                fires.push(i);
            }
        }
        let gaps: Vec<u64> = fires.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            gaps.iter().any(|&g| g > 10),
            "no pause ever stretched a gap: {gaps:?}"
        );
        assert!(gaps.iter().all(|&g| g <= 70), "gap too long: {gaps:?}");
    }

    #[test]
    fn spin_turns_smoothly_with_a_pitch_wave() {
        let _guard = reset();
        state().config.spin = true;
        state().config.mouse_jitter = false;
        quiet_on();
        let mut yaws = Vec::new();
        let mut pitches = Vec::new();
        for _ in 0..40 {
            let out = tick(snap());
            let (yaw, pitch) = out.rotation.expect("spin must set a rotation");
            yaws.push(yaw);
            pitches.push(pitch);
        }
        // Yaw grows every tick and the pitch stays in the wave.
        for w in yaws.windows(2) {
            assert!(w[1] > w[0], "yaw did not grow: {yaws:?}");
        }
        assert!(
            pitches.iter().all(|&p| (-20.0..=20.0).contains(&p)),
            "pitch out of the wave: {pitches:?}"
        );
        // After 40 ticks the yaw velocity has nearly reached 3.6 deg/tick.
        assert!(yaws[39] > 40.0, "spin too slow: {yaws:?}");
    }

    #[test]
    fn mouse_movement_stays_near_its_anchor() {
        let _guard = reset();
        state().config.mouse_movement = true;
        state().config.mouse_jitter = false;
        state().config.mouse_range = 30.0;
        quiet_on();
        for _ in 0..200 {
            let (yaw, pitch) = tick(snap()).rotation.expect("mouse must set a rotation");
            assert!((-30.0..=30.0).contains(&yaw), "yaw left the anchor: {yaw}");
            assert!(
                (-15.0..=15.0).contains(&pitch),
                "pitch left the anchor: {pitch}"
            );
        }
    }

    #[test]
    fn jitter_alone_nudges_only_a_little() {
        let _guard = reset();
        state().config.mouse_jitter = true;
        quiet_on();
        for _ in 0..40 {
            let (yaw, pitch) = tick(snap()).rotation.expect("jitter must set a rotation");
            assert!((-0.8..=0.8).contains(&yaw), "yaw nudge too big: {yaw}");
            assert!(
                (-0.4..=0.4).contains(&pitch),
                "pitch nudge too big: {pitch}"
            );
        }
    }

    #[test]
    fn no_rotation_feature_means_no_rotation() {
        let _guard = reset();
        state().config.mouse_jitter = false;
        quiet_on();
        assert_eq!(tick(snap()).rotation, None);
    }

    #[test]
    fn auto_eat_selects_food_holds_and_restores() {
        let _guard = reset();
        quiet_on();
        let mut s = snap();
        s.food_level = 10;
        s.food_slots = vec![3];
        s.selected_slot = 0;
        let out = tick(s.clone());
        assert_eq!(out.select_slot, Some(3));
        assert_eq!(out.hold_use, Some(true));
        // The adapter selected slot 3, so later snapshots report it.
        s.selected_slot = 3;
        // Eating: the use is held, and after 40 ticks it ends.
        s.using_item = true;
        let mut out = TickOut::default();
        for _ in 0..40 {
            out = tick(s.clone());
        }
        assert_eq!(out.hold_use, Some(false));
        assert_eq!(out.select_slot, Some(0));
        assert!(!state().eating);
    }

    #[test]
    fn auto_eat_skips_when_full_but_runs_behind_a_screen() {
        let _guard = reset();
        quiet_on();
        // Full: nothing.
        let mut s = snap();
        s.food_level = 20;
        s.food_slots = vec![3];
        let out = tick(s.clone());
        assert_eq!(out.select_slot, None);
        assert_eq!(out.hold_use, None);
        // Hungry with a screen open: the original eats behind menus
        // too, so the eat starts.
        s.food_level = 10;
        s.screen_open = true;
        let out = tick(s);
        assert_eq!(out.select_slot, Some(3));
        assert_eq!(out.hold_use, Some(true));
    }

    #[test]
    fn eating_aborts_when_the_item_stops_being_used() {
        let _guard = reset();
        quiet_on();
        let mut s = snap();
        s.food_level = 10;
        s.food_slots = vec![3];
        tick(s.clone());
        assert!(state().eating);
        // Six ticks with no use in progress: the eat is abandoned.
        s.using_item = false;
        let mut out = TickOut::default();
        for _ in 0..6 {
            out = tick(s.clone());
        }
        assert_eq!(out.hold_use, Some(false));
        assert!(!state().eating);
    }

    #[test]
    fn random_hotbar_switches_to_another_slot() {
        let _guard = reset();
        state().config.random_hotbar = true;
        state().config.hotbar_min = 5.0;
        state().config.hotbar_max = 5.0;
        quiet_on();
        let mut seen = None;
        for _ in 0..100 {
            if let Some(slot) = tick(snap()).select_slot {
                seen = Some(slot);
                break;
            }
        }
        let slot = seen.expect("no hotbar switch in 100 ticks");
        assert_ne!(slot, 0);
    }

    #[test]
    fn offhand_swaps_and_swaps_back() {
        let _guard = reset();
        state().config.offhand_swap = true;
        state().config.offhand_min = 10.0;
        state().config.offhand_max = 10.0;
        state().config.offhand_hold = 3.0;
        quiet_on();
        let mut swaps = Vec::new();
        for i in 1..=300u64 {
            if tick(snap()).swap_offhand {
                swaps.push(i);
            }
        }
        assert_eq!(swaps, vec![200, 260], "swaps: {swaps:?}");
    }

    #[test]
    fn inventory_opens_and_closes() {
        let _guard = reset();
        state().config.open_inventory = true;
        state().config.inv_min = 20.0;
        state().config.inv_max = 20.0;
        state().config.inv_hold = 5.0;
        quiet_on();
        let mut opened = None;
        let mut closed = None;
        let mut s = snap();
        for i in 1..=500u64 {
            let out = tick(s.clone());
            if out.open_inventory {
                opened = Some(i);
                s.screen_open = true;
            }
            if out.close_screen {
                closed = Some(i);
            }
        }
        assert_eq!(opened, Some(400));
        assert_eq!(closed, Some(500));
    }

    #[test]
    fn auto_chat_sends_a_listed_message() {
        let _guard = reset();
        state().config.auto_chat = true;
        state().config.chat_messages = vec!["Hey!".to_string(), "Still here!".to_string()];
        state().config.chat_min = 60.0;
        state().config.chat_max = 60.0;
        quiet_on();
        let mut sent = None;
        for _ in 0..1200 {
            if let Some(msg) = tick(snap()).send_chat {
                sent = Some(msg);
                break;
            }
        }
        assert!(
            matches!(sent.as_deref(), Some("Hey!") | Some("Still here!")),
            "sent: {sent:?}"
        );
    }

    #[test]
    fn auto_command_sends_a_listed_command() {
        let _guard = reset();
        state().config.auto_command = true;
        state().config.commands = vec!["/zzz".to_string()];
        state().config.cmd_min = 1.0;
        state().config.cmd_max = 1.0;
        quiet_on();
        let mut sent = None;
        for _ in 0..60 {
            if let Some(msg) = tick(snap()).send_chat {
                sent = Some(msg);
                break;
            }
        }
        assert_eq!(sent.as_deref(), Some("/zzz"));
    }

    #[test]
    fn auto_command_adds_the_slash_and_skips_an_empty_list() {
        let _guard = reset();
        state().config.auto_command = true;
        state().config.commands = vec!["afk-check".to_string()];
        state().config.cmd_min = 1.0;
        state().config.cmd_max = 1.0;
        quiet_on();
        let mut sent = None;
        for _ in 0..60 {
            if let Some(msg) = tick(snap()).send_chat {
                sent = Some(msg);
                break;
            }
        }
        assert_eq!(sent.as_deref(), Some("/afk-check"));
        // Empty list: nothing is sent.
        state().config.commands.clear();
        for _ in 0..60 {
            assert!(tick(snap()).send_chat.is_none());
        }
    }

    #[test]
    fn command_manages_the_command_list() {
        let _guard = reset();
        command("/lmafk cmd add /ping-test").unwrap();
        command("/lmafk cmd add afk-check").unwrap();
        assert_eq!(
            state().config.commands,
            vec![
                "/letmeafk".to_string(),
                "/zzz".to_string(),
                "/afk-check".to_string(),
                "/ping-test".to_string(),
            ]
        );
        command("/lmafk cmd add /zzz").unwrap();
        assert_eq!(state().config.commands.len(), 4);
        let out = command("/lmafk cmd list").unwrap();
        assert!(out.lines.iter().any(|l| l.contains("2: /zzz")));
        command("/lmafk cmd remove 2").unwrap();
        assert_eq!(
            state().config.commands,
            vec![
                "/letmeafk".to_string(),
                "/afk-check".to_string(),
                "/ping-test".to_string(),
            ]
        );
        command("/lmafk cmd clear").unwrap();
        assert!(state().config.commands.is_empty());
    }

    #[test]
    fn auto_disconnect_fires_for_a_nearby_player() {
        let _guard = reset();
        state().config.auto_disconnect = true;
        quiet_on();
        let mut s = snap();
        s.nearby = vec![("Steve".to_string(), [3.0, 64.0, 0.0])];
        let mut out = TickOut::default();
        for _ in 0..10 {
            out = tick(s.clone());
        }
        assert!(
            out.disconnect
                .as_deref()
                .is_some_and(|d| d.contains("Steve")),
            "disconnect: {:?}",
            out.disconnect
        );
    }

    #[test]
    fn disconnect_on_damage_fires_on_a_health_drop() {
        let _guard = reset();
        state().config.disconnect_on_damage = true;
        quiet_on();
        // First tick only records the health.
        let mut s = snap();
        s.health = 20.0;
        assert!(tick(s.clone()).disconnect.is_none());
        // A drop disconnects.
        s.health = 18.0;
        let out = tick(s.clone());
        assert!(
            out.disconnect
                .as_deref()
                .is_some_and(|d| d.contains("damage")),
            "disconnect: {:?}",
            out.disconnect
        );
    }

    #[test]
    fn disconnect_on_damage_ignores_damage_taken_while_off() {
        let _guard = reset();
        state().config.disconnect_on_damage = true;
        quiet_on();
        let mut s = snap();
        s.health = 20.0;
        assert!(tick(s.clone()).disconnect.is_none());
        // Damage taken with the feature off is recorded, not fired.
        state().config.disconnect_on_damage = false;
        s.health = 12.0;
        assert!(tick(s.clone()).disconnect.is_none());
        // Switching it back on does not disconnect for that damage.
        state().config.disconnect_on_damage = true;
        assert!(tick(s).disconnect.is_none());
    }

    #[test]
    fn disconnect_on_damage_ignores_healing_and_stays_off() {
        let _guard = reset();
        state().config.disconnect_on_damage = true;
        quiet_on();
        let mut s = snap();
        s.health = 10.0;
        assert!(tick(s.clone()).disconnect.is_none());
        s.health = 14.0;
        assert!(tick(s.clone()).disconnect.is_none());
        s.health = 14.0;
        assert!(tick(s.clone()).disconnect.is_none());
        // Off: a drop does nothing.
        state().config.disconnect_on_damage = false;
        s.health = 6.0;
        assert!(tick(s).disconnect.is_none());
    }

    #[test]
    fn auto_disconnect_ignores_listed_players_and_far_ones() {
        let _guard = reset();
        state().config.auto_disconnect = true;
        state().config.ignored_players = vec!["steve".to_string()];
        quiet_on();
        let mut s = snap();
        s.nearby = vec![("Steve".to_string(), [3.0, 64.0, 0.0])];
        for _ in 0..20 {
            assert_eq!(tick(s.clone()).disconnect, None);
        }
        // Not ignored, but out of range.
        s.nearby = vec![("Alex".to_string(), [20.0, 64.0, 0.0])];
        for _ in 0..20 {
            assert_eq!(tick(s.clone()).disconnect, None);
        }
    }

    #[test]
    fn auto_logout_warns_at_80_percent_then_disconnects() {
        let _guard = reset();
        state().config.auto_logout = true;
        state().config.logout_minutes = 1.0;
        quiet_on();
        let mut warned = false;
        let mut out = TickOut::default();
        for _ in 0..1200 {
            out = tick(snap());
            if out.lines.iter().any(|l| l.contains("Auto-logout")) {
                warned = true;
            }
            if out.disconnect.is_some() {
                break;
            }
        }
        assert!(warned, "no 80% warning");
        assert!(
            out.disconnect
                .as_deref()
                .is_some_and(|d| d.contains("Auto-logout")),
            "disconnect: {:?}",
            out.disconnect
        );
        assert_eq!(state().afk_ticks, 1200);
    }

    #[test]
    fn hud_line_counts_minutes_and_seconds() {
        let _guard = reset();
        quiet_on();
        wait_ticks(40, snap());
        assert_eq!(hud_line().as_deref(), Some("[AFK] 00:02"));
        wait_ticks(1200, snap());
        assert_eq!(hud_line().as_deref(), Some("[AFK] 01:02"));
    }

    #[test]
    fn hud_line_off_when_toggled_off_or_disabled() {
        let _guard = reset();
        assert_eq!(hud_line(), None);
        quiet_on();
        state().config.hud = false;
        assert_eq!(hud_line(), None);
    }

    #[test]
    fn command_parses_toggles_and_setters() {
        let _guard = reset();
        // Bare /lmafk is the toggle.
        command("/lmafk").unwrap();
        assert!(state().active);
        command("/lmafk toggle").unwrap();
        assert!(!state().active);
        command("/lmafk toggle").unwrap();
        assert!(state().active);
        command("/lmafk interval set 5").unwrap();
        assert_eq!(state().config.action_delay, 5.0);
        assert!(!state().config.random_interval);
        command("/lmafk interval random 1 7").unwrap();
        assert!(state().config.random_interval);
        assert_eq!(state().config.min_secs, 1.0);
        assert_eq!(state().config.max_secs, 7.0);
        command("/lmafk swing").unwrap();
        assert!(state().config.swing);
        command("/lmafk jump").unwrap();
        assert!(!state().config.jump);
        command("/lmafk sneak").unwrap();
        assert!(state().config.sneak);
        command("/lmafk movement").unwrap();
        assert!(state().config.movement);
        command("/lmafk spin 9").unwrap();
        assert_eq!(state().config.spin_speed, 9.0);
        command("/lmafk mousemovement 45").unwrap();
        assert_eq!(state().config.mouse_range, 45.0);
        command("/lmafk eat 10").unwrap();
        assert_eq!(state().config.eat_below, 10);
        command("/lmafk disconnect 16").unwrap();
        assert_eq!(state().config.disconnect_radius, 16.0);
        command("/lmafk logout 30").unwrap();
        assert!(state().config.auto_logout);
        assert_eq!(state().config.logout_minutes, 30.0);
        command("/lmafk logout off").unwrap();
        assert!(!state().config.auto_logout);
        command("/lmafk hud").unwrap();
        assert!(!state().config.hud);
        command("/lmafk jitter").unwrap();
        assert!(state().config.mouse_jitter);
        command("/lmafk pause").unwrap();
        assert!(state().config.random_pause);
        command("/lmafk random").unwrap();
        assert!(!state().config.random_interval);
        // Bare /lmafk toggles off again; status still reports.
        command("/lmafk").unwrap();
        assert!(!state().active);
        let out = command("/lmafk status").unwrap();
        assert!(out.lines.iter().any(|l| l.contains("jump")));
        // Reset restores the original defaults.
        command("/lmafk reset").unwrap();
        assert!(!state().config.swing);
        assert!(!state().config.movement);
        assert_eq!(state().config.action_delay, 5.0);
        assert_eq!(state().config.eat_below, 16);
        assert_eq!(
            state().config.chat_messages,
            vec![
                "Hey!".to_string(),
                "Still here!".to_string(),
                "Just farming!".to_string(),
            ]
        );
        assert!(command("/autofish").is_none());
        assert!(
            command("/lmafk nope")
                .unwrap()
                .lines
                .iter()
                .any(|l| l.contains("/lmafk"))
        );
    }

    #[test]
    fn command_manages_chat_and_ignore_lists() {
        let _guard = reset();
        command("/lmafk chat add Mining!").unwrap();
        command("/lmafk chat add Fishing!").unwrap();
        assert_eq!(
            state().config.chat_messages,
            vec![
                "Hey!".to_string(),
                "Still here!".to_string(),
                "Just farming!".to_string(),
                "Mining!".to_string(),
                "Fishing!".to_string(),
            ]
        );
        let out = command("/lmafk chat list").unwrap();
        assert!(out.lines.iter().any(|l| l.contains("Mining!")));
        command("/lmafk chat add Mining!").unwrap();
        assert_eq!(state().config.chat_messages.len(), 5);
        command("/lmafk chat clear").unwrap();
        assert!(state().config.chat_messages.is_empty());
        command("/lmafk ignore add Steve").unwrap();
        assert!(state().config.is_ignored("STEVE"));
        command("/lmafk ignore remove steve").unwrap();
        assert!(!state().config.is_ignored("Steve"));
        let out = command("/lmafk ignore list").unwrap();
        assert!(out.lines.iter().any(|l| l.contains("empty")));
    }

    #[test]
    fn registered_usages_are_answered() {
        // Every usage the chat tree declares must be one `command()`
        // answers, or typing it would complete and then do nothing.
        let _guard = reset();
        for usage in COMMAND_USAGES {
            let sample = usage
                .replace("<secs:number>", "3")
                .replace("<range:number>", "30")
                .replace("<min:number>", "1")
                .replace("<max:number>", "7")
                .replace("<level:integer>", "14")
                .replace("<radius:number>", "8")
                .replace("<name:word>", "Steve")
                .replace("<message:text>", "hello")
                .replace("<command:text>", "/zzz")
                .replace("<index:integer>", "1")
                .replace("<minutes:integer>", "30");
            assert!(
                command(&format!("/{sample}")).is_some(),
                "registered usage not answered: {usage}"
            );
        }
    }

    #[test]
    fn settings_buttons_cycle() {
        let _guard = reset();
        settings_click(0);
        assert!(!state().config.jump);
        settings_click(13);
        assert_eq!(state().config.action_delay, 7.0);
        settings_click(17);
        assert_eq!(state().config.eat_below, 18);
        settings_click(36);
        assert_eq!(state().config.disconnect_radius, 16.0);
        settings_click(39);
        assert_eq!(state().config.logout_minutes, 60.0);
        assert!(state().config.auto_logout);
        settings_click(39);
        assert_eq!(state().config.logout_minutes, 120.0);
        assert_eq!(settings_buttons().len(), 41);
    }

    #[test]
    fn config_load_falls_back_to_the_default_lists() {
        let _guard = reset();
        // Nothing stored at all: a fresh install.
        load(&|_k| None);
        assert_eq!(
            state().config.chat_messages,
            vec![
                "Hey!".to_string(),
                "Still here!".to_string(),
                "Just farming!".to_string(),
            ]
        );
        assert_eq!(
            state().config.commands,
            vec![
                "/letmeafk".to_string(),
                "/zzz".to_string(),
                "/afk-check".to_string(),
            ]
        );
        assert!(state().config.ignored_players.is_empty());
        // A stored empty value is an explicit clear, not a missing
        // key: it stays empty.
        load(&|k| match k {
            "chat_messages" => Some(String::new()),
            "commands" => Some(String::new()),
            _ => None,
        });
        assert!(state().config.chat_messages.is_empty());
        assert!(state().config.commands.is_empty());
    }

    #[test]
    fn config_load_survives_a_corrupt_store() {
        let _guard = reset();
        load(&|k| match k {
            "hotbar_min" => Some("200".to_string()),
            "hotbar_max" => Some("10".to_string()),
            "spin_speed" => Some("NaN".to_string()),
            "cmd_min" => Some("inf".to_string()),
            _ => None,
        });
        // The min clamps first; the max then clamps against the
        // clamped min, so no f64::clamp panic (min > max).
        assert_eq!(state().config.hotbar_min, 120.0);
        assert_eq!(state().config.hotbar_max, 120.0);
        assert_eq!(state().config.spin_speed, 5.0);
        assert_eq!(state().config.cmd_min, 30.0);
    }

    #[test]
    fn config_load_clamps_bad_values() {
        let _guard = reset();
        load(&|k| match k {
            "action_delay" => Some("500".to_string()),
            "min_secs" => Some("0".to_string()),
            "max_secs" => Some("0".to_string()),
            "eat_below" => Some("99".to_string()),
            "spin_speed" => Some("1".to_string()),
            "chat_messages" => Some("Hey!;Still here!;".to_string()),
            "ignored_players" => Some("Steve;Alex".to_string()),
            _ => None,
        });
        assert_eq!(state().config.action_delay, 60.0);
        assert_eq!(state().config.min_secs, 0.5);
        assert_eq!(state().config.max_secs, 0.5);
        assert_eq!(state().config.eat_below, 20);
        assert_eq!(state().config.spin_speed, 2.0);
        assert_eq!(
            state().config.chat_messages,
            vec!["Hey!".to_string(), "Still here!".to_string()]
        );
        assert_eq!(
            state().config.ignored_players,
            vec!["Steve".to_string(), "Alex".to_string()]
        );
    }
}
