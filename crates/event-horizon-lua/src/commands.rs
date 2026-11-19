//! Command values between the game and Lua: the `event.source` and `event.command` that a mod sees
//! in `run_mod_command`. Both are read-only. [`source_lua`] and [`invocation_lua`] are called by the
//! generated hook (`crate::generated::hooks_impl`), in the turn, so a handle they give is answered
//! by the game only while the turn is open, as every other handle is.
//!
//! A value that has no faithful Lua form is an `Err`: the generated build closure then refuses the
//! delivery for that mod and logs one line. A partial table is never given.

use std::sync::atomic::{AtomicU32, Ordering};

use event_horizon_lua_convert::scalar::ToLuaValue;
use mlua::{AnyUserData, Error, Lua, MultiValue, Table, UserData, Value};
use mod_api::{
    CommandArgumentValue, CommandInvocation, CommandSource, DimensionKey, Host, Tag, Text,
};

use crate::handles::{Family, family_list_out, family_out};
use crate::hostcall::{NOT_HERE, TurnHost};
use crate::stacks;

/// The most handles a list argument (`entities`, `players`) carries to Lua. A longer list refuses
/// the delivery.
pub const MAX_LIST: usize = 256;

/// A `Value` that is a Lua string of `text`.
fn text(lua: &Lua, text: &str) -> mlua::Result<Value> {
    Ok(Value::String(lua.create_string(text)?))
}

/// A table of the named fields, in the given order.
fn named(lua: &Lua, fields: Vec<(&str, Value)>) -> mlua::Result<Value> {
    let table = lua.create_table()?;
    for (key, value) in fields {
        table.raw_set(key, value)?;
    }
    Ok(Value::Table(table))
}

/// `{x = .., y = .., z = ..}` from three numbers.
fn xyz(lua: &Lua, [x, y, z]: [f64; 3]) -> mlua::Result<Value> {
    named(
        lua,
        vec![
            ("x", Value::Number(x)),
            ("y", Value::Number(y)),
            ("z", Value::Number(z)),
        ],
    )
}

/// `"ns:id"` of a dimension. A key the game never gave out is refused.
fn dimension_text(lua: &Lua, host: &dyn Host, key: DimensionKey) -> mlua::Result<Value> {
    let id = key
        .id(host)
        .ok_or_else(|| Error::runtime("an unknown dimension"))?;
    text(lua, &id)
}

/// The `event.source` table of a command: `kind`, the `permission` and what the kind names. A
/// player or a minecart is a handle; a command block's dimension is named through the host.
pub fn source_lua(lua: &Lua, host: &dyn Host, source: &CommandSource) -> mlua::Result<Value> {
    match source {
        CommandSource::Player { player, permission } => named(
            lua,
            vec![
                ("kind", text(lua, "player")?),
                ("permission", permission.to_lua_value(lua)?),
                ("player", family_out(lua, *player)?),
            ],
        ),
        CommandSource::Console { permission } => named(
            lua,
            vec![
                ("kind", text(lua, "console")?),
                ("permission", permission.to_lua_value(lua)?),
            ],
        ),
        CommandSource::CommandBlock {
            position,
            level,
            permission,
        } => named(
            lua,
            vec![
                ("kind", text(lua, "command_block")?),
                ("permission", permission.to_lua_value(lua)?),
                ("position", xyz(lua, position.map(f64::from))?),
                ("level", dimension_text(lua, host, *level)?),
            ],
        ),
        CommandSource::CommandBlockMinecart { entity, permission } => named(
            lua,
            vec![
                ("kind", text(lua, "command_block_minecart")?),
                ("permission", permission.to_lua_value(lua)?),
                ("entity", family_out(lua, *entity)?),
            ],
        ),
        CommandSource::Function { id, permission } => named(
            lua,
            vec![
                ("kind", text(lua, "function")?),
                ("permission", permission.to_lua_value(lua)?),
                ("id", text(lua, &id.to_string())?),
            ],
        ),
        _ => Err(Error::runtime("an unknown command source")),
    }
}

/// A list of handles as a Lua array, refused past [`MAX_LIST`].
fn handles_lua<F: Family>(lua: &Lua, handles: &[F]) -> mlua::Result<Value> {
    if handles.len() > MAX_LIST {
        return Err(Error::runtime(format!(
            "a list is over the cap of {MAX_LIST}"
        )));
    }
    family_list_out(lua, handles.to_vec())
}

/// One argument's Lua value. Every variant either converts whole or is refused.
fn argument_lua(lua: &Lua, host: &dyn Host, value: &CommandArgumentValue) -> mlua::Result<Value> {
    match value {
        CommandArgumentValue::String(value) | CommandArgumentValue::Custom { value, .. } => {
            text(lua, value)
        }
        CommandArgumentValue::Id(id) => text(lua, &id.to_string()),
        CommandArgumentValue::Integer(value) => Ok(Value::Number(f64::from(*value))),
        CommandArgumentValue::Number(value) => Ok(Value::Number(*value)),
        CommandArgumentValue::Boolean(value) => Ok(Value::Boolean(*value)),
        CommandArgumentValue::Player(player) => family_out(lua, *player),
        CommandArgumentValue::Entity(entity) => family_out(lua, *entity),
        CommandArgumentValue::Entities(entities) => handles_lua(lua, entities),
        CommandArgumentValue::Players(players) => handles_lua(lua, players),
        CommandArgumentValue::Item(stack) => {
            // A stack with no id is refused: the spec is missing, not a partial value.
            let spec = stacks::stack_in(host, Some(stack))
                .ok_or_else(|| Error::runtime("a carried value has no id"))?;
            spec.to_lua_value(lua)
        }
        CommandArgumentValue::BlockPos(pos) => xyz(lua, pos.map(f64::from)),
        CommandArgumentValue::Vec3(pos) => xyz(lua, *pos),
        CommandArgumentValue::ColumnPos([x, z]) => named(
            lua,
            vec![
                ("x", Value::Number(f64::from(*x))),
                ("z", Value::Number(f64::from(*z))),
            ],
        ),
        CommandArgumentValue::Rotation([yaw, pitch]) => named(
            lua,
            vec![
                ("yaw", Value::Number(f64::from(*yaw))),
                ("pitch", Value::Number(f64::from(*pitch))),
            ],
        ),
        CommandArgumentValue::Dimension(level) => text(lua, level.dimension_id()),
        CommandArgumentValue::Nbt(compound) => text(
            lua,
            &horizon_world::snbt_text::to_snbt(&Tag::Compound(compound.clone())),
        ),
        _ => Err(Error::runtime("an argument of a kind with no Lua form")),
    }
}

/// The `event.command` table: `literals` (the word path), `args` (each argument by name; the last
/// one of a name wins, as `CommandInvocation::argument` reads it), `dimension` and `position`.
pub fn invocation_lua(
    lua: &Lua,
    host: &dyn Host,
    invocation: &CommandInvocation,
) -> mlua::Result<Value> {
    let literals: Table = lua.create_table()?;
    for (index, word) in invocation.literals.iter().enumerate() {
        literals.raw_set(index + 1, word.as_str())?;
    }
    let args = lua.create_table()?;
    for argument in &invocation.arguments {
        args.raw_set(
            argument.name.as_str(),
            argument_lua(lua, host, &argument.value)?,
        )?;
    }
    named(
        lua,
        vec![
            ("literals", Value::Table(literals)),
            ("args", Value::Table(args)),
            (
                "dimension",
                dimension_text(lua, host, invocation.dimension)?,
            ),
            ("position", xyz(lua, invocation.position)?),
        ],
    )
}

/// The operation `event.feedback` runs through the turn's host. It is a write, so it costs what a
/// write costs.
pub(crate) const FEEDBACK_OP: &str = "feedback";
/// What one `event.feedback` call spends of the turn's budget: a write.
pub(crate) const FEEDBACK_COST: u32 = 10;
/// The most `event.feedback` lines one `run_mod_command` turn may give. A call past it raises.
pub const MAX_FEEDBACK: u32 = 32;
/// The longest `event.feedback` line, in bytes. A longer one raises.
pub const MAX_FEEDBACK_BYTES: usize = 4096;

/// The userdata `event.feedback` passes to the turn's host, one per mod turn: it counts the lines
/// the turn has given so far.
pub(crate) struct FeedbackSink {
    given: AtomicU32,
}

impl UserData for FeedbackSink {}

/// The `feedback` function of a `run_mod_command` event. Each call is a host call of the turn, so it
/// is answered only while the turn is open and its host is a server's; elsewhere it raises.
pub fn feedback_lua(lua: &Lua) -> mlua::Result<Value> {
    let sink = lua.create_userdata(FeedbackSink {
        given: AtomicU32::new(0),
    })?;
    let feedback = lua.create_function(move |lua, text: Value| {
        crate::hostcall::host_call(
            lua,
            FEEDBACK_OP,
            sink.clone(),
            MultiValue::from_vec(vec![text]),
        )
    })?;
    Ok(Value::Function(feedback))
}

/// Runs `event.feedback(text)` for the turn: a plain string of at most `MAX_FEEDBACK_BYTES`, sent
/// to the command's sender as a literal text. Refused past `MAX_FEEDBACK` lines, and when the game
/// refuses the text.
pub(crate) fn feedback_op(
    host: &mut TurnHost<'_>,
    this: &AnyUserData,
    args: MultiValue,
) -> mlua::Result<Value> {
    let Some(server) = host.server() else {
        return Err(Error::runtime(NOT_HERE));
    };
    let sink = this.borrow::<FeedbackSink>()?;
    let text = match args.into_iter().next() {
        Some(Value::String(text)) => text,
        _ => return Err(Error::runtime("feedback needs a string")),
    };
    if text.as_bytes().len() > MAX_FEEDBACK_BYTES {
        return Err(Error::runtime(format!(
            "feedback is longer than {MAX_FEEDBACK_BYTES} bytes"
        )));
    }
    let given = sink.given.load(Ordering::Relaxed);
    if given >= MAX_FEEDBACK {
        return Err(Error::runtime(format!(
            "a turn may give at most {MAX_FEEDBACK} feedback lines"
        )));
    }
    sink.given.store(given + 1, Ordering::Relaxed);
    let line = Text::literal(text.to_str()?.to_owned());
    server
        .commands()
        .feedback(line)
        .map_err(|err| Error::runtime(format!("feedback refused: {err}")))?;
    Ok(Value::Nil)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use mod_api::__private::game::BackendRefOf;
    use mod_api::ServerHost;
    use mod_api::prelude::{LevelsBackend, ServicesServerHost};
    use mod_api::{
        Backend, BackendRef, CommandArgument, Commands, EntityRef, Identifier, LevelRef, Messages,
        PermissionLevel, Players, Tag, TextFormError, WorldEpoch,
    };

    /// The commands service of a test host: the feedback it is given, in order. `refuse` makes
    /// every line refused by the game.
    #[derive(Debug, Default)]
    pub(crate) struct Recorder {
        pub(crate) feedback: Vec<Text>,
        pub(crate) refuse: bool,
    }

    impl Commands for Recorder {
        fn feedback(&mut self, line: Text) -> Result<(), TextFormError> {
            if self.refuse {
                return Err(TextFormError::Malformed);
            }
            self.feedback.push(line);
            Ok(())
        }
    }
    impl Messages for Recorder {}
    impl Players for Recorder {}

    /// A host whose answers are the dimension names, the overworld and the nether (the end is a key
    /// the game never named here, so it has no name), and whose commands service is a [`Recorder`].
    #[derive(Default)]
    pub(crate) struct Dims {
        pub(crate) commands: Recorder,
    }

    impl ServerHost for Dims {}
    mod_api::impl_server_host_areas!(Dims, except(services));
    impl ServicesServerHost for Dims {
        fn commands(&mut self) -> &mut dyn Commands {
            &mut self.commands
        }
        fn messages(&self) -> &dyn Messages {
            &self.commands
        }
        fn players(&self) -> &dyn Players {
            &self.commands
        }
    }

    impl Host for Dims {
        fn backend(&self) -> Option<BackendRef<'_>> {
            Some(BackendRefOf::of(self))
        }
    }
    impl Backend for Dims {}
    mod_api::impl_host_areas!(Dims);
    mod_api::impl_backend_areas!(Dims, except(levels));
    impl LevelsBackend for Dims {
        fn dimension_key_id(&self, key: DimensionKey) -> Option<String> {
            match key {
                DimensionKey::OVERWORLD => Some("minecraft:overworld".to_owned()),
                DimensionKey::NETHER => Some("minecraft:the_nether".to_owned()),
                _ => None,
            }
        }
    }

    fn arg(name: &str, value: CommandArgumentValue) -> CommandArgument {
        CommandArgument::new(name.to_owned(), value)
    }

    fn field(table: &Table, key: &str) -> Value {
        table.get::<Value>(key).expect("a readable field")
    }

    fn text_of(value: &Value) -> String {
        match value {
            Value::String(text) => text.to_string_lossy(),
            other => panic!("expected a string, got {}", other.type_name()),
        }
    }

    fn number_of(value: &Value) -> f64 {
        match value {
            Value::Number(number) => *number,
            Value::Integer(number) => *number as f64,
            other => panic!("expected a number, got {}", other.type_name()),
        }
    }

    fn table_of(value: Value) -> Table {
        match value {
            Value::Table(table) => table,
            other => panic!("expected a table, got {}", other.type_name()),
        }
    }

    fn empty_invocation(arguments: Vec<CommandArgument>) -> CommandInvocation {
        CommandInvocation::new(
            vec!["demo".to_owned(), "run".to_owned()],
            arguments,
            DimensionKey::OVERWORLD,
            [0.5, 64.0, -1.0],
        )
    }

    #[test]
    fn each_source_kind_is_its_own_table() {
        let lua = Lua::new();
        let console = table_of(
            source_lua(
                &lua,
                &Dims::default(),
                &CommandSource::Console {
                    permission: PermissionLevel::Admins,
                },
            )
            .expect("a console converts"),
        );
        assert_eq!(text_of(&field(&console, "kind")), "console");
        assert_eq!(text_of(&field(&console, "permission")), "admins");

        let block = table_of(
            source_lua(
                &lua,
                &Dims::default(),
                &CommandSource::CommandBlock {
                    position: [1, -2, 3],
                    level: DimensionKey::OVERWORLD,
                    permission: PermissionLevel::Moderators,
                },
            )
            .expect("a command block converts"),
        );
        assert_eq!(text_of(&field(&block, "kind")), "command_block");
        assert_eq!(text_of(&field(&block, "permission")), "moderators");
        assert_eq!(text_of(&field(&block, "level")), "minecraft:overworld");
        let at = table_of(field(&block, "position"));
        assert_eq!(number_of(&field(&at, "x")), 1.0);
        assert_eq!(number_of(&field(&at, "y")), -2.0);
        assert_eq!(number_of(&field(&at, "z")), 3.0);

        let function = table_of(
            source_lua(
                &lua,
                &Dims::default(),
                &CommandSource::Function {
                    id: Identifier::parse("demo:run").expect("a valid id"),
                    permission: PermissionLevel::All,
                },
            )
            .expect("a function converts"),
        );
        assert_eq!(text_of(&field(&function, "kind")), "function");
        assert_eq!(text_of(&field(&function, "id")), "demo:run");
        assert_eq!(text_of(&field(&function, "permission")), "all");

        let minecart = table_of(
            source_lua(
                &lua,
                &Dims::default(),
                &CommandSource::CommandBlockMinecart {
                    entity: EntityRef::new(WorldEpoch::new(1), 9),
                    permission: PermissionLevel::Owners,
                },
            )
            .expect("a minecart converts"),
        );
        assert_eq!(text_of(&field(&minecart, "kind")), "command_block_minecart");
        assert!(matches!(field(&minecart, "entity"), Value::UserData(_)));
    }

    #[test]
    fn each_argument_kind_has_its_lua_value() {
        let lua = Lua::new();
        let tag = match Tag::compound([("a", Tag::Int(1))]) {
            Tag::Compound(compound) => compound,
            other => panic!("expected a compound, got {other:?}"),
        };
        let invocation = empty_invocation(vec![
            arg("text", CommandArgumentValue::String("hi there".to_owned())),
            arg("n", CommandArgumentValue::Integer(-3)),
            arg("x", CommandArgumentValue::Number(2.5)),
            arg("b", CommandArgumentValue::Boolean(true)),
            arg("pos", CommandArgumentValue::BlockPos([1, 2, 3])),
            arg("vec", CommandArgumentValue::Vec3([0.5, 1.0, 2.0])),
            arg("col", CommandArgumentValue::ColumnPos([4, 5])),
            arg("rot", CommandArgumentValue::Rotation([90.0, -45.0])),
            arg(
                "dim",
                CommandArgumentValue::Dimension(LevelRef::new(
                    WorldEpoch::new(1),
                    DimensionKey::NETHER,
                )),
            ),
            arg("tag", CommandArgumentValue::Nbt(tag)),
            arg(
                "custom",
                CommandArgumentValue::Custom {
                    kind: "demo:kind".to_owned(),
                    value: "v".to_owned(),
                },
            ),
            arg(
                "id",
                CommandArgumentValue::Id(Identifier::parse("demo:item").expect("a valid id")),
            ),
            arg(
                "list",
                CommandArgumentValue::Entities(vec![
                    EntityRef::new(WorldEpoch::new(1), 1),
                    EntityRef::new(WorldEpoch::new(1), 2),
                ]),
            ),
            arg("dup", CommandArgumentValue::String("first".to_owned())),
            arg("dup", CommandArgumentValue::String("second".to_owned())),
        ]);
        let table =
            table_of(invocation_lua(&lua, &Dims::default(), &invocation).expect("it converts"));

        let literals = table_of(field(&table, "literals"));
        assert_eq!(
            text_of(&literals.raw_get::<Value>(1).expect("a word")),
            "demo"
        );
        assert_eq!(
            text_of(&literals.raw_get::<Value>(2).expect("a word")),
            "run"
        );
        assert_eq!(text_of(&field(&table, "dimension")), "minecraft:overworld");
        assert_eq!(
            number_of(&field(&table_of(field(&table, "position")), "y")),
            64.0
        );

        let args = table_of(field(&table, "args"));
        assert_eq!(text_of(&field(&args, "text")), "hi there");
        assert_eq!(number_of(&field(&args, "n")), -3.0);
        assert_eq!(number_of(&field(&args, "x")), 2.5);
        assert!(matches!(field(&args, "b"), Value::Boolean(true)));
        let pos = table_of(field(&args, "pos"));
        assert_eq!(number_of(&field(&pos, "z")), 3.0);
        let vec = table_of(field(&args, "vec"));
        assert_eq!(number_of(&field(&vec, "z")), 2.0);
        let col = table_of(field(&args, "col"));
        assert_eq!(number_of(&field(&col, "x")), 4.0);
        assert_eq!(number_of(&field(&col, "z")), 5.0);
        let rot = table_of(field(&args, "rot"));
        assert_eq!(number_of(&field(&rot, "yaw")), 90.0);
        assert_eq!(number_of(&field(&rot, "pitch")), -45.0);
        assert_eq!(text_of(&field(&args, "dim")), "minecraft:the_nether");
        assert_eq!(text_of(&field(&args, "tag")), "{a:1}");
        assert_eq!(text_of(&field(&args, "custom")), "v");
        assert_eq!(text_of(&field(&args, "id")), "demo:item");
        assert_eq!(table_of(field(&args, "list")).raw_len(), 2);
        assert_eq!(text_of(&field(&args, "dup")), "second");
    }

    #[test]
    fn a_list_over_the_cap_or_a_dimension_with_no_name_is_refused() {
        let lua = Lua::new();
        let at_cap = vec![EntityRef::new(WorldEpoch::new(1), 1); MAX_LIST];
        let over_cap = vec![EntityRef::new(WorldEpoch::new(1), 1); MAX_LIST + 1];
        let ok = empty_invocation(vec![arg("l", CommandArgumentValue::Entities(at_cap))]);
        assert!(invocation_lua(&lua, &Dims::default(), &ok).is_ok());
        let too_long = empty_invocation(vec![arg("l", CommandArgumentValue::Entities(over_cap))]);
        assert!(invocation_lua(&lua, &Dims::default(), &too_long).is_err());

        let unnamed = CommandInvocation::new(
            vec!["demo".to_owned()],
            Vec::new(),
            DimensionKey::END,
            [0.0; 3],
        );
        assert!(invocation_lua(&lua, &Dims::default(), &unnamed).is_err());
        let block = CommandSource::CommandBlock {
            position: [0, 0, 0],
            level: DimensionKey::END,
            permission: PermissionLevel::All,
        };
        assert!(source_lua(&lua, &Dims::default(), &block).is_err());
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::MAX_FEEDBACK;
    use super::tests::Dims;
    use crate::discover::{Discovered, ENTRY, Side};
    use crate::host::tests::{captured_with_prefix, install_capture};
    use crate::host::{LuaHost, LuaMod};
    use mod_api::Text;
    use mod_api::{
        CommandArgument, CommandArgumentValue, CommandInvocation, CommandSource, DimensionKey,
        EntityRef, Flow, Hooks, PermissionLevel, RunModCommandEvent, WorldEpoch,
    };
    use std::path::Path;

    /// Loads mod `id` whose main.luau is `source`, on both sides.
    fn load(root: &Path, id: &str, source: &str) -> LuaMod {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(ENTRY), source).unwrap();
        LuaMod::load(&Discovered {
            dir,
            id: id.to_string(),
            version: "1".to_string(),
            side: Side::Both,
        })
        .unwrap()
    }

    fn console() -> CommandSource {
        CommandSource::Console {
            permission: PermissionLevel::Admins,
        }
    }

    fn invocation() -> CommandInvocation {
        CommandInvocation::new(
            vec!["demo".to_owned(), "run".to_owned()],
            vec![CommandArgument::new(
                "n".to_owned(),
                CommandArgumentValue::Integer(3),
            )],
            DimensionKey::OVERWORLD,
            [0.0; 3],
        )
    }

    /// Fires `run_mod_command` at `host` with the given source and invocation, as the game does.
    fn run(host: &LuaHost, source: &CommandSource, command: &CommandInvocation) -> Flow {
        let mut event = RunModCommandEvent {
            source,
            command,
            cancelled: false,
        };
        Hooks::run_mod_command(host, &mut Dims::default(), &mut event)
    }

    const RUN: &str = "horizon.on(\"run_mod_command\", function(event) ";

    #[test]
    fn setting_ran_true_cancels_the_command() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "ran_true",
            &format!("{RUN}event.ran = true end)"),
        );
        let host = LuaHost::new(vec![m]);
        assert!(matches!(
            run(&host, &console(), &invocation()),
            Flow::Cancel
        ));
    }

    #[test]
    fn an_absent_or_false_ran_continues() {
        let root = tempfile::tempdir().unwrap();
        let absent = load(
            root.path(),
            "ran_absent",
            &format!("{RUN}horizon.log(\"seen\") end)"),
        );
        let host = LuaHost::new(vec![absent]);
        assert!(matches!(
            run(&host, &console(), &invocation()),
            Flow::Continue
        ));
        assert_eq!(host.mods()[0].log_lines(), ["seen"]);

        let off = load(
            root.path(),
            "ran_false",
            &format!("{RUN}event.ran = false end)"),
        );
        let host = LuaHost::new(vec![off]);
        assert!(matches!(
            run(&host, &console(), &invocation()),
            Flow::Continue
        ));
    }

    #[test]
    fn a_console_source_and_its_literals_reach_the_handler() {
        let root = tempfile::tempdir().unwrap();
        let source = format!(
            "{RUN}horizon.log(event.source.kind .. \"|\" .. event.source.permission .. \"|\" .. \
             event.command.literals[1] .. \"|\" .. event.command.literals[2] .. \"|\" .. \
             event.command.dimension .. \"|\" .. tostring(event.command.args.n)) end)"
        );
        let host = LuaHost::new(vec![load(root.path(), "console_fields", &source)]);
        run(&host, &console(), &invocation());
        assert_eq!(
            host.mods()[0].log_lines(),
            ["console|admins|demo|run|minecraft:overworld|3"]
        );
    }

    #[test]
    fn a_non_boolean_ran_is_refused_with_one_warning_and_the_command_continues() {
        let root = tempfile::tempdir().unwrap();
        let id = "ran_text";
        install_capture();
        let host = LuaHost::new(vec![load(
            root.path(),
            id,
            &format!("{RUN}event.ran = \"yes\" end)"),
        )]);
        assert!(matches!(
            run(&host, &console(), &invocation()),
            Flow::Continue
        ));
        let lines = captured_with_prefix(&format!("{id}: run_mod_command"));
        assert_eq!(lines.len(), 1, "got {lines:?}");
        assert!(lines[0].contains("refused"));
    }

    #[test]
    fn an_error_in_the_handler_voids_its_turn() {
        let root = tempfile::tempdir().unwrap();
        let source = format!("{RUN}event.ran = true error(\"boom\") end)");
        let host = LuaHost::new(vec![load(root.path(), "ran_error", &source)]);
        assert!(matches!(
            run(&host, &console(), &invocation()),
            Flow::Continue
        ));
    }

    #[test]
    fn a_mod_that_did_not_subscribe_is_not_called() {
        let root = tempfile::tempdir().unwrap();
        let other = "horizon.on(\"block_receive_game\", function() horizon.log(\"wrong\") end)";
        let host = LuaHost::new(vec![load(root.path(), "not_subscribed", other)]);
        assert!(matches!(
            run(&host, &console(), &invocation()),
            Flow::Continue
        ));
        assert!(host.mods()[0].log_lines().is_empty());
    }

    #[test]
    fn a_refused_over_cap_list_is_logged_once_however_often_it_repeats() {
        let root = tempfile::tempdir().unwrap();
        let id = "flood_refusals";
        install_capture();
        let host = LuaHost::new(vec![load(
            root.path(),
            id,
            &format!("{RUN}event.ran = true end)"),
        )]);
        let over_cap = |n: usize| {
            CommandInvocation::new(
                vec!["demo".to_owned()],
                vec![CommandArgument::new(
                    "l".to_owned(),
                    CommandArgumentValue::Entities(vec![EntityRef::new(WorldEpoch::new(1), 1); n]),
                )],
                DimensionKey::OVERWORLD,
                [0.0; 3],
            )
        };
        for n in [257, 300, 400] {
            assert!(matches!(
                run(&host, &console(), &over_cap(n)),
                Flow::Continue
            ));
        }
        let lines = captured_with_prefix(&format!("{id}: run_mod_command"));
        assert_eq!(lines.len(), 1, "got {lines:?}");
        assert!(lines[0].contains("over the cap"));
    }

    /// Fires `run_mod_command` at `host` with `dims` open, so the test can read what the game was given.
    fn run_with(host: &LuaHost, dims: &mut Dims) -> Flow {
        let source = console();
        let command = invocation();
        let mut event = RunModCommandEvent {
            source: &source,
            command: &command,
            cancelled: false,
        };
        Hooks::run_mod_command(host, dims, &mut event)
    }

    /// A host whose `run_mod_command` handler runs `body` (with `event` in scope) and then ends.
    fn feedback_host(root: &Path, id: &str, body: &str) -> LuaHost {
        LuaHost::new(vec![load(root, id, &format!("{RUN}{body} end)"))])
    }

    #[test]
    fn feedback_reaches_the_commands_service_once_per_call() {
        let root = tempfile::tempdir().unwrap();
        let host = feedback_host(
            root.path(),
            "fb_once",
            "event.feedback(\"got 3\") event.ran = true",
        );
        let mut dims = Dims::default();
        assert!(matches!(run_with(&host, &mut dims), Flow::Cancel));
        assert_eq!(dims.commands.feedback, vec![Text::literal("got 3")]);
    }

    #[test]
    fn the_33rd_feedback_line_of_a_turn_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let host = feedback_host(
            root.path(),
            "fb_flood",
            "for i = 1, 40 do event.feedback(\"line \" .. i) end",
        );
        let mut dims = Dims::default();
        assert!(matches!(run_with(&host, &mut dims), Flow::Continue));
        let lines: Vec<Text> = (1..=MAX_FEEDBACK)
            .map(|i| Text::literal(format!("line {i}")))
            .collect();
        assert_eq!(dims.commands.feedback, lines);
    }

    #[test]
    fn feedback_is_capped_in_bytes_and_must_be_a_string() {
        let root = tempfile::tempdir().unwrap();
        let mut dims = Dims::default();
        let long = feedback_host(
            root.path(),
            "fb_long",
            "event.feedback(string.rep(\"a\", 4097))",
        );
        assert!(matches!(run_with(&long, &mut dims), Flow::Continue));
        assert!(dims.commands.feedback.is_empty());

        let at_cap = feedback_host(
            root.path(),
            "fb_cap",
            "event.feedback(string.rep(\"a\", 4096)) event.ran = true",
        );
        assert!(matches!(run_with(&at_cap, &mut dims), Flow::Cancel));
        assert_eq!(dims.commands.feedback.len(), 1);

        let number = feedback_host(root.path(), "fb_number", "event.feedback(3)");
        assert!(matches!(run_with(&number, &mut dims), Flow::Continue));
        assert_eq!(dims.commands.feedback.len(), 1);
    }

    #[test]
    fn a_line_the_game_refuses_fails_the_call_and_voids_the_turn() {
        let root = tempfile::tempdir().unwrap();
        let host = feedback_host(
            root.path(),
            "fb_refused",
            "event.feedback(\"x\") event.ran = true",
        );
        let mut dims = Dims::default();
        dims.commands.refuse = true;
        assert!(matches!(run_with(&host, &mut dims), Flow::Continue));
        assert!(dims.commands.feedback.is_empty());
    }

    #[test]
    fn feedback_called_with_no_turn_open_raises_outside_a_hook() {
        let lua = mlua::Lua::new();
        let feedback: mlua::Function = match crate::commands::feedback_lua(&lua).unwrap() {
            mlua::Value::Function(function) => function,
            other => panic!("expected a function, got {}", other.type_name()),
        };
        let err = feedback.call::<()>("late").unwrap_err().to_string();
        assert!(err.contains("no host call outside a hook"), "got {err}");
    }
}
