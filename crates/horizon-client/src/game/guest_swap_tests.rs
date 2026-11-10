//! The acting guest's records during their turn (issue 49): `native_level_players`,
//! `native_player_sneaking` / `native_player_eye_height` and
//! `native_entity_data_with_attachments` must name the player whose body is
//! swapped in, never the world owner, and never twice.
use super::{
    Audio, Game, Options, Progress, Settings, execution_role::ExecutionRole, load_resources,
};
use horizon_client::{
    lan::{Joined, NetConfig, PlayerSender, guest::Guest, hub::HubConfig, playerdata::ints_uuid},
    motion::PlayerMotion,
    pose::Pose,
};
use horizon_core::geometry::Vec3;
use horizon_protocol::{crypto::offline_uuid, packets::configuration::ClientInformation};
use horizon_world::{
    attachments::{ATTACHMENTS_KEY, Attachments},
    nbt::{Compound, JavaString, Tag},
};
use std::sync::{Arc, atomic::AtomicI32, mpsc};

const GUEST_CONN: u32 = 2;
const GUEST_UUID: u128 = 2;
const GUEST_NAME: &str = "SwapGuest";
const GUEST_ENTITY: i32 = 1001;
const GUEST_POS: [f64; 3] = [22.5, 101.0, 9.5];

/// A host on a stone floor with one connected, placed guest (conn 2), as a
/// dedicated server runs: no serving state, no remote client.
fn game() -> Game<'static> {
    let root = std::path::PathBuf::from(
        std::env::var_os("HORIZON_TEST_RESOURCES")
            .expect("set HORIZON_TEST_RESOURCES to prepared resources"),
    );
    let settings = Settings::default();
    let loaded = load_resources(
        root.clone(),
        None,
        &root,
        &settings,
        std::time::Instant::now(),
        false,
        true,
        &Progress::new(),
        &mut |_, _| unreachable!(),
    )
    .unwrap();
    let mut options = Options::parse(std::iter::empty()).unwrap().unwrap();
    options.resources = root.clone();
    options.player_name = Some("Owner".into());
    options.execution_role = ExecutionRole::IntegratedServer;
    let res: &'static _ = Box::leak(Box::new(loaded.resources));
    let mut game = Game::new(
        res,
        loaded.pack,
        loaded.gui.unwrap(),
        options,
        settings,
        Audio::silent(&root),
        None,
    )
    .unwrap();
    game.build_registries();
    game.player = Some(PlayerMotion::new(game.motion, Vec3::new(4.5, 100.0, 6.5)).unwrap());
    game.lan_host(
        super::lan::LanSettings {
            port: 0,
            guest_commands: true,
            force_game_mode: false,
        },
        NetConfig {
            announce: false,
            ..NetConfig::lan(0, "swap-test".into(), 2)
        },
        HubConfig {
            default_game_mode: 1,
            guest_commands: true,
            force_game_mode: false,
            hardcore: false,
            view_distance: 2,
            simulation_distance: 2,
            dimension: super::Dim::Overworld.id().into(),
            allow_flight: false,
            seed: 0,
            max_players: 2,
            integrated: false,
            levels: Default::default(),
        },
    )
    .unwrap();
    let (tx, _rx) = mpsc::channel();
    let mut guest = Guest::new(
        Joined {
            conn: GUEST_CONN,
            uuid: GUEST_UUID,
            name: GUEST_NAME.into(),
            properties: Vec::new(),
            info: ClientInformation::new(2),
            brand: "test".into(),
            sender: PlayerSender::new(tx, Arc::default()),
            latency: Arc::new(AtomicI32::new(0)),
            address: "127.0.0.1:25565".parse().unwrap(),
            expected: false,
            configured: Default::default(),
        },
        GUEST_ENTITY,
        GUEST_POS,
        2,
    );
    guest.placed = true;
    guest.loaded = true;
    guest.dimension = super::Dim::Overworld.id().into();
    guest.permission = 2;
    game.lan.as_mut().unwrap().hub.guests.push(guest);
    game
}

/// The single-player world's owner, as `native_server` names them: no
/// serving state here, so the saved uuid, else the offline one of "Player".
fn owner_uuid(game: &Game<'_>) -> u128 {
    assert!(
        game.serving.is_none(),
        "a dedicated-style game has no serving state"
    );
    game.level
        .player_uuid
        .map(ints_uuid)
        .unwrap_or_else(|| offline_uuid("Player"))
}

fn attachments_with(key: &str) -> Attachments {
    let mut raw = Compound::default();
    raw.insert(JavaString::from(key), Tag::Double(1.0f64.to_bits()));
    Attachments::from_nbt(raw)
}

/// A guest's use/break runs with the hub held out: `native_level_players`
/// must name the one swapped-in body for who it is (the acting guest), not
/// as the owner with permission 4.
#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn level_players_names_the_acting_guest_while_the_hub_is_out() {
    let mut game = game();
    let owner = owner_uuid(&game);
    assert_ne!(owner, GUEST_UUID);
    let mut host = game.lan.take().unwrap();
    let swap = game
        .enter_guest(&mut host, GUEST_CONN, game.dim)
        .expect("enter");
    let players = game.native_level_players();
    assert_eq!(
        players.len(),
        1,
        "one record: the acting guest, never the owner: {players:?}"
    );
    let p = &players[0];
    assert_eq!(
        p.uuid, GUEST_UUID,
        "owner {owner} mislabels the guest's body: {p:?}"
    );
    assert_eq!(p.name, GUEST_NAME);
    assert_eq!(
        p.permission, 2,
        "the guest's own permission, not the owner's 4"
    );
    assert_eq!(p.position, GUEST_POS);
    game.leave_guest(&mut host, swap);
    game.lan = Some(host);
}

/// On a guest command the hub is back in place: the acting guest is already
/// listed, so no second record may copy their live body under the owner's
/// identity.
#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn level_players_lists_the_acting_guest_once_when_the_hub_is_back() {
    let mut game = game();
    let owner = owner_uuid(&game);
    let mut host = game.lan.take().unwrap();
    let swap = game
        .enter_guest(&mut host, GUEST_CONN, game.dim)
        .expect("enter");
    // What a guest command restores first (`run_guest_command`).
    game.lan = Some(host);
    let players = game.native_level_players();
    assert_eq!(
        players.len(),
        1,
        "the acting guest, listed once: {players:?}"
    );
    assert_eq!(players[0].uuid, GUEST_UUID);
    assert!(
        players.iter().all(|p| p.uuid != owner),
        "no owner-labelled copy of the guest's body: {players:?}"
    );
    let mut host = game.lan.take().expect("the command puts the hub back");
    game.leave_guest(&mut host, swap);
    game.lan = Some(host);
}

/// The acting guest crouches: asking for the `level.dat` owner (or the
/// offline uuid of "Player") on a server with the guest swapped in must not
/// answer with the guest's pose.
#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn sneaking_and_eye_height_answer_only_the_uuid_asked() {
    let mut game = game();
    let owner = owner_uuid(&game);
    let mut host = game.lan.take().unwrap();
    let swap = game
        .enter_guest(&mut host, GUEST_CONN, game.dim)
        .expect("enter");
    let body = game.player.as_mut().expect("the guest's body");
    body.pose = Pose::Crouching;
    body.dimensions.eye_height = 1.27;
    assert_eq!(
        game.native_player_sneaking(owner),
        None,
        "the owner is not on this server turn"
    );
    assert_eq!(
        game.native_player_eye_height(owner),
        1.62,
        "unknown players read standing height"
    );
    assert_eq!(
        game.native_player_sneaking(GUEST_UUID),
        Some(true),
        "the acting guest's own pose still reads"
    );
    game.leave_guest(&mut host, swap);
    game.lan = Some(host);
}

/// With the hub back during the guest's turn, `horizon:attachments` for the
/// acting guest is their own: `enter_guest` swapped it into
/// `player_attachments`, leaving the host's under their uuid in the map.
#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn entity_data_with_attachments_reads_the_acting_guests_own() {
    let mut game = game();
    game.guest_attachments
        .insert(GUEST_UUID, attachments_with("guest_load"));
    game.player_attachments = attachments_with("host_load");
    let mut host = game.lan.take().unwrap();
    let swap = game
        .enter_guest(&mut host, GUEST_CONN, game.dim)
        .expect("enter");
    // What a guest command restores first (`run_guest_command`).
    game.lan = Some(host);
    let shown = game
        .native_entity_data_with_attachments(
            super::Dim::Overworld.id(),
            u32::try_from(GUEST_ENTITY).unwrap(),
        )
        .expect("the acting guest's data reads");
    let held = shown
        .get(&JavaString::from(ATTACHMENTS_KEY))
        .expect("attachments are shown");
    let Tag::Compound(map) = held else {
        panic!("attachments read as a compound, got {held:?}");
    };
    assert!(
        map.contains_key(&JavaString::from("guest_load")),
        "the acting guest's own attachments, got {held:?}"
    );
    assert!(
        !map.contains_key(&JavaString::from("host_load")),
        "the host's attachments leaked into the guest's data: {held:?}"
    );
    let mut host = game.lan.take().expect("the command puts the hub back");
    game.leave_guest(&mut host, swap);
    game.lan = Some(host);
}
