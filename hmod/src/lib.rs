//! LetMeAFK for Horizon (native .hmod): the hooks that feed the
//! pure-logic engine in `engine.rs`.
//!
//! The engine keeps all state behind one mutex; the hooks do the game's I/O:
//! - `client_tick_pre` reads the local player (look, ground, hunger,
//!   hotbar food, nearby players), runs the engine, and acts on its
//!   answer: rotation through `set_local_rotation`, hotbar selection,
//!   holding the use key to eat, inventory opens, swing and offhand
//!   swaps through raw packets, punch and use through the game's own
//!   click mappings, chat messages and auto-commands, disconnects;
//! - `movement_input` applies the engine's wanted keys (the WASD walk
//!   cycle, timed jump, sneak toggle, random pauses) — it leaves the
//!   input alone while the mod is toggled off, so vanilla movement
//!   stands until the player turns it on;
//! - `input_key` toggles the mod on the registered keybind (default F8,
//!   rebindable in Controls — the bound key is read back through
//!   `mod_key`, so a rebind keeps working);
//! - `render_gui_post` draws the `[AFK] mm:ss` timer in the top-left
//!   corner while the mod is on;
//! - `command` answers `/lmafk`;
//! - `register_client_commands` declares `/lmafk` and its usages to
//!   the chat tree, so typing it colours and completes like the game's
//!   own commands instead of showing red as unknown;
//! - `register` declares the settings on the mod's page and the toggle
//!   keybind;
//! - `client_player_logging_in` resets the run per connection;
//! - `client_disconnecting` records the last server's address, so the
//!   auto-reconnect in a later version is one call site (there is no
//!   `client_connect` in the mod API yet, so 1.0.0 does not reconnect).
//!
//! The registered settings hold the values the settings page edits.
//! The chat messages, the command list and the ignore list live in the
//! mod's data files (`horizon-mod-data/letmeafk/`): `chat_messages`,
//! `commands` and `ignored_players` hold the semicolon-joined lists,
//! so the commands can still change them.

use mod_api::{
    self, ClientDisconnectingEvent, ClientHost, ClientPlayerLoggingInEvent, ClientTickPreEvent,
    Command, CommandEvent, ConfigPath, Corner, EntityRef, Flow, Hooks, Identifier, InputKeyEvent,
    KeyAction, KeyRegistration, MovementInputEvent, RegisterClientCommandsEvent, Registrar,
    RegistrationContext, RenderGuiPostEvent, ServerboundPlayPacket, SettingChangedEvent,
    SettingSpec, Text,
};
use std::sync::Mutex;

mod engine;

/// The settings the mod's page edits; the two lists live in data
/// files of the same names, so the commands can still change them.
const REGISTERED_SETTINGS: [&str; 41] = [
    "jump",
    "sneak",
    "swing",
    "punch",
    "use_item",
    "movement",
    "spin",
    "mouse_movement",
    "mouse_jitter",
    "random_pause",
    "random_interval",
    "auto_eat",
    "random_hotbar",
    "offhand_swap",
    "open_inventory",
    "auto_chat",
    "auto_command",
    "auto_disconnect",
    "disconnect_on_damage",
    "auto_logout",
    "hud",
    "action_delay",
    "min_secs",
    "max_secs",
    "spin_speed",
    "mouse_range",
    "eat_below",
    "hotbar_min",
    "hotbar_max",
    "offhand_min",
    "offhand_max",
    "offhand_hold",
    "inv_min",
    "inv_max",
    "inv_hold",
    "chat_min",
    "chat_max",
    "cmd_min",
    "cmd_max",
    "disconnect_radius",
    "logout_minutes",
];

const CHAT_FILE: &str = "chat_messages";
const CMD_FILE: &str = "commands";
const IGNORED_FILE: &str = "ignored_players";
const LAST_SERVER_FILE: &str = "last_server";

/// The toggle keybind's registration id and its default key.
const TOGGLE_KEY_ID: &str = "toggle_afk";
const TOGGLE_KEY_DEFAULT: &str = "key.keyboard.f8";

/// The player's own inventory menu, as the client shows it: 46 slots
/// (crafting result, grid and armour, then the main inventory and the
/// hotbar).
const INVENTORY_MENU_SLOTS: usize = 46;

/// The `minecraft:swing` packet's hand: the main hand.
const MAIN_HAND: u32 = 0;
/// The `minecraft:player_action` action for swapping with the offhand.
const SWAP_WITH_OFFHAND: u32 = 6;
/// The `minecraft:player_action` direction for the swap: down.
const DIRECTION_DOWN: u32 = 0;
/// The auto-disconnect box half-size: the largest radius the settings
/// page allows, so the engine's own radius check does the filtering.
const NEARBY_BOX: f64 = 64.0;

#[derive(Default)]
struct Mod {
    /// Whether a failed packet send has been told to the player this
    /// session, so a refused send does not spam chat every tick.
    send_failed: Mutex<bool>,
}

/// One data file as text, if it exists and is UTF-8.
fn read_data(host: &dyn ClientHost, name: &str) -> Option<String> {
    let path = ConfigPath::new(name).ok()?;
    let bytes = host.data_files().read(&path).ok()?;
    String::from_utf8(bytes).ok()
}

/// Replace one data file with `text`.
fn write_data(host: &dyn ClientHost, name: &str, text: &str) {
    if let Ok(path) = ConfigPath::new(name) {
        let _ = host.data_files().write(&path, text.as_bytes());
    }
}

/// Read the settings and data files into the engine. The registered settings
/// come from the host's typed reads; the two lists come from data files.
fn load(host: &dyn ClientHost) {
    let chat = read_data(host, CHAT_FILE);
    let commands = read_data(host, CMD_FILE);
    let ignored = read_data(host, IGNORED_FILE);
    engine::load(&|key| match key {
        "jump"
        | "sneak"
        | "swing"
        | "punch"
        | "use_item"
        | "movement"
        | "spin"
        | "mouse_movement"
        | "mouse_jitter"
        | "random_pause"
        | "random_interval"
        | "auto_eat"
        | "random_hotbar"
        | "offhand_swap"
        | "open_inventory"
        | "auto_chat"
        | "auto_command"
        | "auto_disconnect"
        | "disconnect_on_damage"
        | "auto_logout"
        | "hud" => host.setting_bool(key).map(|b| b.to_string()),
        "action_delay" | "min_secs" | "max_secs" | "spin_speed" | "mouse_range" | "eat_below"
        | "hotbar_min" | "hotbar_max" | "offhand_min" | "offhand_max" | "offhand_hold"
        | "inv_min" | "inv_max" | "inv_hold" | "chat_min" | "chat_max" | "cmd_min" | "cmd_max"
        | "disconnect_radius" | "logout_minutes" => host.setting_f64(key).map(|f| format!("{f}")),
        "chat_messages" => chat.clone(),
        "commands" => commands.clone(),
        "ignored_players" => ignored.clone(),
        _ => None,
    });
}

/// Write the engine's queued store writes: registered settings through the
/// host, the two lists through their data files.
fn persist(host: &dyn ClientHost) {
    let store = engine::take_store();
    if store.is_empty() {
        return;
    }
    for (key, value) in store {
        if key == CHAT_FILE {
            write_data(host, CHAT_FILE, &value);
        } else if key == CMD_FILE {
            write_data(host, CMD_FILE, &value);
        } else if key == IGNORED_FILE {
            write_data(host, IGNORED_FILE, &value);
        } else if REGISTERED_SETTINGS.contains(&key.as_str()) {
            host.set_setting(engine::ID, &key, &value);
        }
    }
}

/// A `minecraft:` id, or None when it does not parse.
fn ident(text: &str) -> Option<Identifier> {
    Identifier::parse(text).ok()
}

/// Whether the player holds food in hotbar slot `slot` (0..9), read
/// from the inventory menu: the hotbar sits at menu slots 36..45.
fn slot_is_food(host: &dyn ClientHost, menu: &mod_api::MenuRef, slot: usize) -> bool {
    let Some(Some(stack)) = menu.slot(host, 36 + slot) else {
        return false;
    };
    host.item_default_component(stack.item(), "minecraft:food")
        .is_some()
}

/// What the adapter saw this tick. None when no player is in a world.
fn snapshot(host: &mut dyn ClientHost) -> Option<engine::Snapshot> {
    let mut snap = engine::Snapshot::default();
    let player = host.local_player()?;
    let (yaw, pitch) = host
        .camera()
        .map(|c| (c.yaw, c.pitch))
        .unwrap_or((0.0, 0.0));
    snap.yaw = yaw;
    snap.pitch = pitch;
    // The entity reads are methods on the refs, not on the host: the
    // host traits only carry the backend methods the game implements.
    // `PlayerRef` is `Copy`, so `player.into()` can make the entity
    // handle again wherever one is needed.
    let entity: EntityRef = player.into();
    snap.on_ground = entity.on_ground(&*host).unwrap_or(true);
    snap.screen_open = !host.playing() || host.observed_screen_id().is_some();
    snap.food_level = player.food(&*host).map(|f| f.level).unwrap_or(-1);
    snap.health = host
        .living(entity)
        .and_then(|living| living.health(&*host))
        .unwrap_or(-1.0);
    snap.selected_slot = player.selected_slot(&*host).unwrap_or(0);
    if let Some(menu) = player.container_menu(&*host)
        && menu.slot_count(&*host) == Some(INVENTORY_MENU_SLOTS)
    {
        for slot in 0..9usize {
            if slot_is_food(host, &menu, slot) {
                snap.food_slots.push(slot);
            }
        }
    }
    let pos = entity.position(&*host)?;
    snap.player_pos = pos;
    // Nearby players: every entity in the box whose type is the
    // player type, excluding the local player (same position — a
    // second player cannot stand on the exact same spot). The name
    // is the entity's name, which for a player is the profile name.
    // `entity_uuid` is server-side only, so it cannot tell the local
    // player from another here.
    if let Some(level) = player.level(&*host) {
        let min = [
            pos[0] - NEARBY_BOX,
            pos[1] - NEARBY_BOX,
            pos[2] - NEARBY_BOX,
        ];
        let max = [
            pos[0] + NEARBY_BOX,
            pos[1] + NEARBY_BOX,
            pos[2] + NEARBY_BOX,
        ];
        if let Some(entities) = level.entities_in(&*host, min, max) {
            for other in entities {
                let is_player = other
                    .kind(&*host)
                    .map(|kind| kind.id(&*host) == "minecraft:player")
                    .unwrap_or(false);
                if !is_player {
                    continue;
                }
                if let Some(epos) = other.position(&*host) {
                    if epos == pos {
                        continue;
                    }
                    if let Some(name) = other.name(&*host) {
                        snap.nearby.push((name, epos));
                    }
                }
            }
        }
    }
    snap.using_item = host.local_item_use().is_some();
    Some(snap)
}

/// A Minecraft protocol VarInt (LEB128, seven bits a byte).
fn write_varint(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Send one raw packet, reporting the first refusal of the session to
/// the player once. False when the packet could not be sent.
fn send_packet(
    host: &mut dyn ClientHost,
    id_text: &str,
    body: Vec<u8>,
    reported: &Mutex<bool>,
) -> bool {
    let Some(id) = ident(id_text) else {
        return false;
    };
    match host.send_packet(ServerboundPlayPacket::new(id, body)) {
        Ok(()) => true,
        Err(error) => {
            let mut reported = reported.lock().unwrap_or_else(|e| e.into_inner());
            if !*reported {
                *reported = true;
                let _ = host.chat_line(Text::literal(format!(
                    "[LetMeAFK] Could not send the {id_text} packet: {error:?}"
                )));
            }
            false
        }
    }
}

/// Swing the main hand: a `minecraft:swing` packet, VarInt hand.
/// Not `click_mapping("key.attack")` — that starts a real attack and
/// could hit an entity under the crosshair.
fn send_swing(host: &mut dyn ClientHost, reported: &Mutex<bool>) -> bool {
    let mut body = Vec::with_capacity(1);
    write_varint(&mut body, MAIN_HAND);
    send_packet(host, "minecraft:swing", body, reported)
}

/// The body of a `minecraft:player_action` packet: VarInt action, a
/// BlockPos (one packed i64, big-endian), VarInt direction, VarInt
/// sequence. The sequence is 0 here, as vanilla sends for the
/// offhand swap (and Geyser does for non-dig actions): the server
/// only uses it to acknowledge block-break predictions. An earlier
/// build left the sequence out entirely, so the packet was truncated
/// and the server never swapped — the swap silently did nothing.
fn player_action_body(action: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(17);
    write_varint(&mut body, action);
    body.extend_from_slice(&0i64.to_be_bytes());
    write_varint(&mut body, DIRECTION_DOWN);
    write_varint(&mut body, 0);
    body
}

/// Send a `minecraft:player_action` packet. The swap uses action 6
/// (`SWAP_ITEM_WITH_OFFHAND`), BlockPos.ZERO and direction down —
/// exactly the packet vanilla sends for the offhand key.
fn send_player_action(host: &mut dyn ClientHost, action: u32, reported: &Mutex<bool>) -> bool {
    send_packet(
        host,
        "minecraft:player_action",
        player_action_body(action),
        reported,
    )
}

/// The toggle keybind's bound key, as Controls shows it — the saved
/// binding, or the default when there is none.
fn toggle_key(host: &dyn ClientHost) -> String {
    host.mod_key(engine::ID, TOGGLE_KEY_ID)
        .map(|(key, _)| key)
        .unwrap_or_else(|| TOGGLE_KEY_DEFAULT.to_string())
}

#[mod_api::horizon_mod]
impl Hooks for Mod {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        // The page is one flat list the game renders in
        // registration order — the API has no sections or folders —
        // so the settings are registered in clusters, each range
        // right under the feature it belongs to: actions, camera,
        // timing, eating, items, chat, safety, display.
        // Actions.
        let _ = r.register_setting(SettingSpec::toggle("jump", "Auto jump", true));
        let _ = r.register_setting(SettingSpec::toggle("sneak", "Sneak mode", false));
        let _ = r.register_setting(SettingSpec::toggle("swing", "Swing hand", false));
        let _ = r.register_setting(SettingSpec::toggle("punch", "Punch", false));
        let _ = r.register_setting(SettingSpec::toggle("use_item", "Use item", false));
        let _ = r.register_setting(SettingSpec::toggle("movement", "Player movement", false));
        // Camera.
        let _ = r.register_setting(SettingSpec::toggle("spin", "Auto spin", false));
        let _ = r.register_setting(SettingSpec::range(
            "spin_speed",
            "Spin speed (seconds per turn)",
            2.0,
            30.0,
            1.0,
            5.0,
        ));
        let _ = r.register_setting(SettingSpec::toggle(
            "mouse_movement",
            "Mouse movement",
            false,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "mouse_range",
            "Mouse movement range",
            5.0,
            90.0,
            5.0,
            60.0,
        ));
        let _ = r.register_setting(SettingSpec::toggle("mouse_jitter", "Mouse jitter", false));
        // Timing.
        let _ = r.register_setting(SettingSpec::toggle("random_pause", "Random pauses", false));
        let _ = r.register_setting(SettingSpec::toggle(
            "random_interval",
            "Random intervals",
            false,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "action_delay",
            "Action delay",
            0.5,
            60.0,
            0.5,
            5.0,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "min_secs",
            "Random interval min",
            0.5,
            60.0,
            0.5,
            3.0,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "max_secs",
            "Random interval max",
            0.5,
            60.0,
            0.5,
            7.0,
        ));
        // Eating.
        let _ = r.register_setting(SettingSpec::toggle("auto_eat", "Auto eat", true));
        let _ = r.register_setting(SettingSpec::range(
            "eat_below",
            "Eat below hunger",
            1.0,
            20.0,
            1.0,
            16.0,
        ));
        // Items.
        let _ = r.register_setting(SettingSpec::toggle("random_hotbar", "Random hotbar", false));
        let _ = r.register_setting(SettingSpec::range(
            "hotbar_min",
            "Hotbar min",
            1.0,
            120.0,
            1.0,
            5.0,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "hotbar_max",
            "Hotbar max",
            1.0,
            300.0,
            1.0,
            15.0,
        ));
        let _ = r.register_setting(SettingSpec::toggle("offhand_swap", "Offhand swap", false));
        let _ = r.register_setting(SettingSpec::range(
            "offhand_min",
            "Offhand min",
            1.0,
            300.0,
            1.0,
            15.0,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "offhand_max",
            "Offhand max",
            1.0,
            600.0,
            1.0,
            45.0,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "offhand_hold",
            "Offhand hold",
            1.0,
            30.0,
            1.0,
            3.0,
        ));
        let _ = r.register_setting(SettingSpec::toggle(
            "open_inventory",
            "Open inventory",
            false,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "inv_min",
            "Inventory min",
            1.0,
            300.0,
            1.0,
            10.0,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "inv_max",
            "Inventory max",
            1.0,
            600.0,
            1.0,
            25.0,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "inv_hold",
            "Inventory hold",
            1.0,
            30.0,
            1.0,
            2.0,
        ));
        // Chat.
        let _ = r.register_setting(SettingSpec::toggle(
            "auto_chat",
            "Auto chat messages",
            false,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "chat_min", "Chat min", 1.0, 1200.0, 1.0, 30.0,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "chat_max", "Chat max", 1.0, 3600.0, 1.0, 120.0,
        ));
        let _ = r.register_setting(SettingSpec::toggle("auto_command", "Auto command", false));
        let _ = r.register_setting(SettingSpec::range(
            "cmd_min",
            "Command min",
            1.0,
            1200.0,
            1.0,
            30.0,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "cmd_max",
            "Command max",
            1.0,
            3600.0,
            1.0,
            120.0,
        ));
        // Safety.
        let _ = r.register_setting(SettingSpec::toggle(
            "auto_disconnect",
            "Auto disconnect",
            false,
        ));
        let _ = r.register_setting(SettingSpec::range(
            "disconnect_radius",
            "Disconnect radius",
            1.0,
            64.0,
            1.0,
            10.0,
        ));
        let _ = r.register_setting(SettingSpec::toggle(
            "disconnect_on_damage",
            "Disconnect on damage",
            false,
        ));
        let _ = r.register_setting(SettingSpec::toggle("auto_logout", "Auto logout", false));
        let _ = r.register_setting(SettingSpec::range(
            "logout_minutes",
            "Auto-logout after (minutes)",
            0.0,
            240.0,
            5.0,
            30.0,
        ));
        // Display.
        let _ = r.register_setting(SettingSpec::toggle("hud", "AFK timer HUD", true));
        r.register_key_mapping(
            KeyRegistration::new(TOGGLE_KEY_ID, TOGGLE_KEY_DEFAULT).label("Toggle LetMeAFK"),
        );
    }

    fn client_tick_pre(&self, host: &mut dyn ClientHost, _event: &mut ClientTickPreEvent) {
        if !host.on(engine::ID) {
            // Off: forget the run, so turning it back on does not
            // resume a stale walk cycle or eat.
            engine::reset();
            return;
        }
        load(host);
        let Some(snap) = snapshot(host) else {
            // No player in a world: the run is over until the player
            // joins again and toggles on.
            engine::reset();
            return;
        };
        let out = engine::tick(snap);
        if let Some((yaw, pitch)) = out.rotation {
            let _ = host.set_local_rotation(yaw, pitch);
        }
        if let Some(slot) = out.select_slot {
            let _ = host.select_hotbar_slot(slot);
        }
        if let Some(hold) = out.hold_use {
            let _ = host.set_mapping_down("key.use", hold);
        }
        if out.open_inventory {
            host.open_inventory();
        }
        if out.close_screen {
            host.close_screen();
        }
        if out.swing {
            send_swing(host, &self.send_failed);
        }
        if out.punch {
            // One click of the attack mapping: the game attacks
            // whatever is under the crosshair, as the player's own
            // click would. Dropped while a screen is open.
            let _ = host.click_mapping("key.attack");
        }
        if out.use_item {
            // One click of the use mapping: the held item's own use.
            let _ = host.click_mapping("key.use");
        }
        if out.swap_offhand {
            send_player_action(host, SWAP_WITH_OFFHAND, &self.send_failed);
        }
        if let Some(message) = out.send_chat {
            let _ = host.send_chat(&message);
        }
        if let Some(reason) = out.disconnect {
            let _ = host.disconnect(Some(Text::literal(reason)));
        }
        for line in out.lines {
            let _ = host.chat_line(Text::literal(line));
        }
        persist(host);
    }

    fn movement_input(&self, host: &dyn ClientHost, event: &mut MovementInputEvent) {
        if !host.on(engine::ID) || !engine::is_active() {
            // Toggled off: leave the player's own input alone.
            return;
        }
        let keys = engine::wanted_keys();
        event.forward_key = keys.forward;
        event.backward_key = keys.backward;
        event.left_key = keys.left;
        event.right_key = keys.right;
        event.jump = keys.jump;
        event.sneak = keys.sneak;
        // `sprint`, `forward` and `strafe` are left untouched: the
        // mod never sprints, and the axes follow the keys.
    }

    fn input_key(&self, host: &mut dyn ClientHost, event: &mut InputKeyEvent<'_>) {
        if !host.on(engine::ID) {
            return;
        }
        if event.action != KeyAction::Press {
            return;
        }
        let bound = toggle_key(host);
        if event.key != bound.as_str() {
            return;
        }
        load(host);
        let active = engine::toggle();
        let _ = host.chat_line(Text::literal(format!(
            "[LetMeAFK] LetMeAFK {}.",
            if active { "On" } else { "Off" }
        )));
        persist(host);
    }

    fn render_gui_post(&self, host: &dyn ClientHost, event: &mut RenderGuiPostEvent<'_>) {
        if !host.on(engine::ID) {
            return;
        }
        let Some(line) = engine::hud_line() else {
            return;
        };
        let width = event.draw.text_width(&line);
        let (x, y) = event.corners.place(Corner::TopLeft, width, 10.0);
        event.draw.text(&line, [x, y], 0xFFFFFFFF, true);
    }

    fn register_client_commands(
        &self,
        _host: &mut dyn ClientHost,
        event: &mut RegisterClientCommandsEvent<'_>,
    ) {
        // Declare every usage to the chat tree (Starling 0.9.0's
        // `register_client_commands`, NeoForge's
        // `RegisterClientCommandsEvent`): without this the client has
        // no `/lmafk` in its tree, so typing it shows red as an
        // unknown command with no completion, even though the
        // `command` hook below still answers it after send. The
        // usages come from the engine, so the tree cannot drift from
        // what the mod actually runs. Asked at connect, never per
        // frame; level 0 throughout, the only level a client command
        // may have.
        event
            .commands
            .extend(engine::COMMAND_USAGES.iter().map(|u| Command::new(u)));
    }

    fn command(&self, host: &mut dyn ClientHost, event: &mut CommandEvent<'_>) -> Flow {
        if !host.on(engine::ID) {
            return Flow::Continue;
        }
        load(host);
        let Some(out) = engine::command(event.command) else {
            return Flow::Continue;
        };
        for line in out.lines {
            let _ = host.chat_line(Text::literal(line));
        }
        persist(host);
        Flow::Cancel
    }

    fn setting_changed(&self, host: &mut dyn ClientHost, _event: &mut SettingChangedEvent<'_>) {
        if !host.on(engine::ID) {
            return;
        }
        load(host);
    }

    fn client_player_logging_in(
        &self,
        host: &mut dyn ClientHost,
        _event: &mut ClientPlayerLoggingInEvent,
    ) {
        if let Ok(mut reported) = self.send_failed.lock() {
            *reported = false;
        }
        if !host.on(engine::ID) {
            return;
        }
        load(host);
        engine::reset();
    }

    fn client_disconnecting(&self, host: &dyn ClientHost, _event: &mut ClientDisconnectingEvent) {
        // Record the server just left, so a later version with a
        // `client_connect` can offer to rejoin it. There is no such
        // call in the mod API yet, so 1.0.0 only records it.
        if let Some(server) = host.client_session().client_server()
            && let Some(address) = server.address
        {
            write_data(host, LAST_SERVER_FILE, &address);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_action_body_has_the_sequence() {
        // VarInt 6 (swap), BlockPos.ZERO (8 bytes), VarInt 0
        // (direction down), VarInt 0 (sequence). Leaving the
        // sequence out truncated the packet and the server never
        // swapped — this is the regression test for that.
        assert_eq!(
            player_action_body(SWAP_WITH_OFFHAND),
            vec![0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x00]
        );
    }
}
