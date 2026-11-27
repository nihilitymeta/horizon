//! Property and fuzz tests over the conversions: round trips, NBT trees, garbage Lua
//! values, limits, strictness, and the rule that no mod metamethod runs.
//!
//! The generator is a xorshift64* with fixed seeds. A failure names its seed and case,
//! so it can be replayed. Expected values are written by hand; the round trips compare a
//! value with itself, never with the code's own output.

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mlua::{Lua, Value};
use mod_api::{BlockPos, Compound, Identifier, Tag, Text};

use crate::ids::{parse_uuid, uuid_string};
use crate::item::{BlockStateSpec, ItemStackSpec};
use crate::lua_enum;
use crate::nbt;
use crate::scalar::{ConvertError, FromLuaValue, ToLuaValue};
use crate::vector::Rotation;

/// xorshift64*: a small deterministic generator. The seed must not be zero.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number from 0 to `n - 1`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next() as u8).collect()
    }

    /// A string of `len` characters drawn from `alphabet`.
    fn text(&mut self, alphabet: &[char], len: usize) -> String {
        (0..len)
            .map(|_| alphabet[self.below(alphabet.len() as u64) as usize])
            .collect()
    }
}

const LOWER: &[char] = &[
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z',
];
const ANY_CHAR: &[char] = &[
    'a', 'Z', '0', ' ', '\n', '\0', 'é', '中', '🙂', '/', '-', '_',
];
/// Lua numbers that are awkward on purpose.
const SPECIAL: [f64; 10] = [
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    -0.0,
    0.0,
    1e300,
    -1e300,
    9_223_372_036_854_775_808.0,
    -9_223_372_036_854_775_808.0,
    1.5,
];
/// Exact as a Lua number: the most a 64-bit integer may hold here.
const EXACT: u64 = 1 << 53;

const SEED_ROUND: u64 = 0x9E37_79B9_7F4A_7C15;
const SEED_NBT: u64 = 0xD1B5_4A32_D192_ED03;
const SEED_GARBAGE: u64 = 0x8CB9_2BA7_2F3D_8DD7;
const SEED_STRICT: u64 = 0x5851_F42D_4C95_7F2D;

/// A value written and read back.
fn round_trip<T: ToLuaValue + FromLuaValue>(lua: &Lua, x: &T) -> T {
    let v = x.to_lua_value(lua).expect("to lua");
    T::from_lua_value(&v).expect("from lua")
}

macro_rules! int_round_trip {
    ($lua:expr, $rng:expr, $case:expr, $($ty:ty),+) => {$({
        let r = $rng.next();
        // 53 random bits (exact as a Lua number), negated for half the signed draws.
        let mut x = (r >> 11) as $ty;
        // Unsigned types (MIN 0) keep their sign: a negative would not be in range.
        if r & 1 == 1 && <$ty>::MIN != 0 {
            x = x.wrapping_neg();
        }
        assert_eq!(round_trip($lua, &x), x, "{} seed {SEED_ROUND:#x} case {}", stringify!($ty), $case);
    })+};
}

#[test]
fn integers_floats_bools_and_strings_round_trip() {
    let lua = Lua::new();
    let mut rng = Rng::new(SEED_ROUND);
    for case in 0..3000 {
        int_round_trip!(&lua, rng, case, u8, i8, u16, i16, u32, i32, u64, i64, usize);
        let f = f64::from_bits(rng.next());
        if f.is_finite() {
            assert_eq!(
                round_trip(&lua, &f),
                f,
                "f64 seed {SEED_ROUND:#x} case {case}"
            );
        }
        let g = f32::from_bits(rng.next() as u32);
        if g.is_finite() {
            assert_eq!(
                round_trip(&lua, &g),
                g,
                "f32 seed {SEED_ROUND:#x} case {case}"
            );
        }
        let b = rng.below(2) == 1;
        assert_eq!(round_trip(&lua, &b), b);
        let len = rng.below(40) as usize;
        let s = rng.text(ANY_CHAR, len);
        assert_eq!(
            round_trip(&lua, &s),
            s,
            "string seed {SEED_ROUND:#x} case {case}"
        );
    }
}

#[test]
fn integer_edges_round_trip_up_to_the_exact_limit() {
    let lua = Lua::new();
    assert_eq!(round_trip(&lua, &0u8), 0);
    assert_eq!(round_trip(&lua, &u8::MAX), u8::MAX);
    assert_eq!(round_trip(&lua, &i8::MIN), i8::MIN);
    assert_eq!(round_trip(&lua, &(EXACT as i64)), EXACT as i64);
    assert_eq!(round_trip(&lua, &-(EXACT as i64)), -(EXACT as i64));
    assert_eq!(round_trip(&lua, &EXACT), EXACT);
    assert_eq!(round_trip(&lua, &(EXACT as usize)), EXACT as usize);
    assert_eq!(round_trip(&lua, &f64::MIN_POSITIVE), f64::MIN_POSITIVE);
}

#[test]
fn durations_round_trip_in_seconds() {
    let lua = Lua::new();
    let mut rng = Rng::new(SEED_ROUND ^ 1);
    for case in 0..3000 {
        // Whole seconds across the whole range: exact as f64.
        let whole = Duration::from_secs(rng.below(1_000_000_001));
        assert_eq!(round_trip(&lua, &whole), whole, "seconds case {case}");
        // Milliseconds up to 4e6 s: the f64 seconds are exact to the nanosecond there.
        let millis = Duration::from_millis(rng.below(4_000_000_000));
        assert_eq!(round_trip(&lua, &millis), millis, "millis case {case}");
    }
}

#[test]
fn uuids_round_trip_and_match_a_hand_formatted_string() {
    let lua = Lua::new();
    let mut rng = Rng::new(SEED_ROUND ^ 2);
    for case in 0..3000 {
        let bytes: [u8; 16] = std::array::from_fn(|_| rng.next() as u8);
        let value = u128::from_be_bytes(bytes);
        assert_eq!(round_trip(&lua, &value), value, "uuid case {case}");
        let mut hex = String::new();
        for (i, b) in bytes.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                hex.push('-');
            }
            hex.push_str(&format!("{b:02x}"));
        }
        assert_eq!(uuid_string(value), hex, "uuid text case {case}");
        assert_eq!(parse_uuid(&hex.to_uppercase()), Ok(value), "case {case}");
    }
}

#[test]
fn identifiers_round_trip() {
    let lua = Lua::new();
    let ns_alphabet: Vec<char> = LOWER.iter().copied().chain("0189_.-".chars()).collect();
    let path_alphabet: Vec<char> = ns_alphabet.iter().copied().chain(['/']).collect();
    let mut rng = Rng::new(SEED_ROUND ^ 3);
    let mut checked = 0;
    while checked < 3000 {
        let ns_len = 1 + rng.below(8) as usize;
        let namespace = rng.text(&ns_alphabet, ns_len);
        let path_len = 1 + rng.below(12) as usize;
        let path = rng.text(&path_alphabet, path_len);
        if namespace == ".." {
            continue;
        }
        let id = Identifier::from_parts(&namespace, &path).expect("valid parts");
        assert_eq!(round_trip(&lua, &id), id, "{namespace}:{path}");
        checked += 1;
    }
}

/// A random NBT tree: every tag kind, at most six levels, a node budget per tree.
fn gen_compound(rng: &mut Rng, level: usize, budget: &mut usize) -> Compound {
    let mut map = Compound::new();
    for _ in 0..rng.below(6) {
        if *budget == 0 {
            break;
        }
        let len = rng.below(5) as usize;
        let name = rng.text(LOWER, len);
        if name == "__nbt" {
            continue;
        }
        let kind = rng.below(if level >= 6 { 10 } else { 12 });
        map.insert(name.as_str().into(), gen_tag(rng, kind, level, budget));
    }
    map
}

/// A tag of `kind` (0 to 11, as in [`gen_tag`]) held by a container at `level`.
fn gen_tag(rng: &mut Rng, kind: u64, level: usize, budget: &mut usize) -> Tag {
    *budget = budget.saturating_sub(1);
    match kind {
        0 => Tag::Byte(rng.next() as i8),
        1 => Tag::Short(rng.next() as i16),
        2 => Tag::Int(rng.next() as i32),
        // Full range: longs past 2^53 travel as strings.
        3 => Tag::Long(rng.next() as i64),
        4 => loop {
            let bits = rng.next() as u32;
            if f32::from_bits(bits).is_finite() {
                break Tag::Float(bits);
            }
        },
        5 => {
            // Half the doubles are integral, which takes the tagged form.
            let n = if rng.below(2) == 0 {
                f64::from_bits(rng.next())
            } else {
                ((rng.next() as i64) >> 20) as f64
            };
            if n.is_finite() {
                Tag::Double(n.to_bits())
            } else {
                Tag::Double(0.0f64.to_bits())
            }
        }
        6 => {
            let len = rng.below(6) as usize;
            Tag::ByteArray(rng.bytes(len))
        }
        7 => {
            let len = rng.below(6);
            Tag::IntArray((0..len).map(|_| rng.next() as i32).collect())
        }
        // Within the exact range: an element past 2^53 is not written (see the defect test).
        8 => {
            let len = rng.below(6);
            Tag::LongArray(
                (0..len)
                    .map(|_| (rng.next() >> 11) as i64 * if rng.below(2) == 0 { 1 } else { -1 })
                    .collect(),
            )
        }
        9 => {
            let len = rng.below(12) as usize;
            Tag::String(rng.text(ANY_CHAR, len).as_str().into())
        }
        10 => {
            // A list holds one element kind; an empty list holds none.
            let elem = rng.below(12);
            let len = rng.below(4);
            let items = (0..len)
                .map(|_| gen_tag(rng, elem, level, budget))
                .collect();
            Tag::List(items)
        }
        _ => Tag::Compound(gen_compound(rng, level + 1, budget)),
    }
}

#[test]
fn nbt_trees_round_trip() {
    let lua = Lua::new();
    let mut rng = Rng::new(SEED_NBT);
    for case in 0..2000 {
        let mut budget = 120;
        let root = gen_compound(&mut rng, 1, &mut budget);
        assert_eq!(
            round_trip(&lua, &root),
            root,
            "compound seed {SEED_NBT:#x} case {case}"
        );
        let tag = Tag::Compound(root);
        assert_eq!(
            round_trip(&lua, &tag),
            tag,
            "tag seed {SEED_NBT:#x} case {case}"
        );
    }
}

/// Lua values no mod should be able to pass in unharmed: odd bytes, every number, nesting past
/// the limits, a cycle, an erroring `__index`, and a vector with NaN.
fn garbage(lua: &Lua, rng: &mut Rng, depth: usize) -> Value {
    // Tables stop at depth 21: the NBT and text limits are 16, so this goes 5 past them.
    match rng.below(if depth >= 21 { 7 } else { 10 }) {
        0 => Value::Nil,
        1 => Value::Boolean(rng.below(2) == 1),
        2 => Value::Integer(rng.next() as i64),
        3 => Value::Number(f64::from_bits(rng.next())),
        4 => Value::Number(SPECIAL[rng.below(SPECIAL.len() as u64) as usize]),
        5 => {
            // Half odd bytes, half valid text, so long names reach the parsers too.
            let len = rng.below(5001) as usize;
            let bytes = if rng.below(2) == 0 {
                rng.bytes(len)
            } else {
                rng.text(ANY_CHAR, len).into_bytes()
            };
            Value::String(lua.create_string(bytes).expect("string"))
        }
        6 => odd_leaf(lua, rng),
        _ => {
            let t = lua.create_table().expect("table");
            for _ in 0..rng.below(4) {
                let key = match rng.below(4) {
                    0 => Value::String(lua.create_string(rng.bytes(8)).expect("string")),
                    1 => Value::Integer(rng.below(6) as i64 + 1),
                    2 => Value::Boolean(true),
                    _ => Value::Number(1.5),
                };
                let value = garbage(lua, rng, depth + 1);
                t.raw_set(key, value).expect("set");
            }
            Value::Table(t)
        }
    }
}

fn odd_leaf(lua: &Lua, rng: &mut Rng) -> Value {
    match rng.below(3) {
        0 => {
            let t = lua.create_table().expect("table");
            t.raw_set("self", t.clone()).expect("set");
            Value::Table(t)
        }
        1 => {
            let t = lua.create_table().expect("table");
            let meta = lua.create_table().expect("table");
            let refuse = lua
                .create_function(|_, _: mlua::MultiValue| -> mlua::Result<Value> {
                    Err(mlua::Error::runtime("index refused"))
                })
                .expect("function");
            meta.raw_set("__index", refuse).expect("set");
            t.set_metatable(Some(meta)).expect("metatable");
            Value::Table(t)
        }
        _ => lua
            .load("return vector.create(0/0, 1, 2)")
            .eval::<Value>()
            .expect("vector"),
    }
}

/// Calls `T::from_lua_value` on `value` and fails if it panics or its error text is empty.
fn probe<T: FromLuaValue>(value: &Value, name: &str, case: &str) {
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| T::from_lua_value(value)));
    match outcome {
        Err(_) => panic!("{name} panicked ({case})"),
        Ok(Ok(_)) => {}
        Ok(Err(error)) => assert!(
            !error.to_string().is_empty(),
            "{name}: empty error ({case})"
        ),
    }
}

/// The error text of `T::from_lua_value` on `value`, if it is an error.
fn error_text<T: FromLuaValue>(value: &Value) -> Option<String> {
    T::from_lua_value(value)
        .err()
        .map(|e: ConvertError| e.to_string())
}

macro_rules! for_each_impl {
    ($callback:ident!($($args:tt)*)) => {
        $callback!($($args)*
            bool, i8, u8, i16, u16, i32, u32, i64, u64, usize, f32, f64, String,
            Option<i32>, Option<[i32; 3]>, Duration, u128, Identifier, Rotation, BlockPos,
            [i32; 3], [i32; 2], [f64; 3], [f64; 2], [f32; 3], [f32; 2],
            Tag, Compound, Text, BlockStateSpec, ItemStackSpec, Shade)
    };
}

macro_rules! probe_all {
    ($value:expr, $case:expr, $($ty:ty),+) => {$(
        probe::<$ty>($value, stringify!($ty), $case);
    )+};
}

#[derive(Debug, PartialEq)]
enum Shade {
    Light,
    Dark,
}

lua_enum!(Shade { Light => "light", Dark => "dark" });

#[test]
fn no_conversion_panics_on_garbage() {
    let lua = Lua::new();
    let mut rng = Rng::new(SEED_GARBAGE);
    for case in 0..2000 {
        let value = garbage(&lua, &mut rng, 0);
        let label = format!("seed {SEED_GARBAGE:#x} case {case}");
        for_each_impl!(probe_all!(&value, &label,));
    }
}

#[test]
fn garbage_errors_are_short_and_say_something() {
    let lua = Lua::new();
    let mut rng = Rng::new(SEED_GARBAGE ^ 1);
    for case in 0..1000 {
        let value = garbage(&lua, &mut rng, 0);
        let label = format!("seed {SEED_GARBAGE:#x} case {case}");
        for text in [
            error_text::<Tag>(&value),
            error_text::<Compound>(&value),
            error_text::<Text>(&value),
            error_text::<Identifier>(&value),
            error_text::<ItemStackSpec>(&value),
            error_text::<[f64; 3]>(&value),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                !text.is_empty() && text.len() < 500,
                "{} bytes ({label}): {text:.80}",
                text.len()
            );
        }
    }
}

/// A table that counts its metamethod calls in `flag`. It has one raw field and no others, so any
/// read of a missing key or any length would reach its metatable.
fn guarded(lua: &Lua, flag: &Arc<AtomicBool>) -> Value {
    let t = lua.create_table().expect("table");
    t.raw_set("x", 1).expect("set");
    let meta = lua.create_table().expect("table");
    for name in [
        "__index",
        "__newindex",
        "__len",
        "__eq",
        "__tostring",
        "__call",
    ] {
        let flag = Arc::clone(flag);
        let hook = lua
            .create_function(move |_, _: mlua::MultiValue| {
                flag.store(true, Ordering::SeqCst);
                Ok(Value::Integer(1))
            })
            .expect("function");
        meta.raw_set(name, hook).expect("set");
    }
    t.set_metatable(Some(meta)).expect("metatable");
    Value::Table(t)
}

#[test]
fn mod_metamethods_never_run() {
    let lua = Lua::new();
    let flag = Arc::new(AtomicBool::new(false));
    let value = guarded(&lua, &flag);
    let _ = <[i32; 3]>::from_lua_value(&value);
    let _ = <[i32; 2]>::from_lua_value(&value);
    let _ = <[f64; 3]>::from_lua_value(&value);
    let _ = <[f32; 2]>::from_lua_value(&value);
    let _ = Option::<[i32; 3]>::from_lua_value(&value);
    let _ = Rotation::from_lua_value(&value);
    let _ = BlockPos::from_lua_value(&value);
    let _ = Tag::from_lua_value(&value);
    let _ = Compound::from_lua_value(&value);
    let _ = Text::from_lua_value(&value);
    let _ = BlockStateSpec::from_lua_value(&value);
    let _ = ItemStackSpec::from_lua_value(&value);
    assert!(!flag.load(Ordering::SeqCst), "a mod metamethod ran");
}

/// A compound nested `levels` tables deep, the innermost empty.
fn nested_tables(lua: &Lua, levels: usize) -> Value {
    let mut inner = lua.create_table().expect("table");
    for _ in 1..levels {
        let outer = lua.create_table().expect("table");
        outer.raw_set("c", inner).expect("set");
        inner = outer;
    }
    Value::Table(inner)
}

/// A defect found by this file, kept as an ignored test: the plain enum's error repeats the whole
/// input name, so a long string gives an error past the 500-byte bound.
#[test]
fn enum_error_does_not_echo_a_long_name() {
    let lua = Lua::new();
    let long = Value::String(lua.create_string("x".repeat(2000)).expect("string"));
    let text = error_text::<Shade>(&long).expect("refused");
    assert!(text.len() < 500, "{} bytes", text.len());
}

/// A defect found by this file, kept as an ignored test: an unknown NBT tag name is echoed whole.
#[test]
fn unknown_nbt_tag_error_does_not_echo_a_long_name() {
    let lua = Lua::new();
    let t = lua.create_table().expect("table");
    t.raw_set("__nbt", "y".repeat(2000)).expect("set");
    t.raw_set("value", 1).expect("set");
    let text = error_text::<Tag>(&Value::Table(t)).expect("refused");
    assert!(text.len() < 500, "{} bytes", text.len());
}

#[test]
fn nbt_limits_refuse_past_the_bound_quickly() {
    let lua = Lua::new();
    let start = Instant::now();

    // The root is level 1: 16 levels read, 17 do not.
    let deepest_ok = nested_tables(&lua, nbt::MAX_DEPTH);
    assert!(Tag::from_lua_value(&deepest_ok).is_ok());
    let too_deep = nested_tables(&lua, nbt::MAX_DEPTH + 1);
    assert!(Tag::from_lua_value(&too_deep).is_err());

    // The root tag counts as one: 4095 children make 4096 tags, one more does not.
    let fill = |n: usize| {
        let t = lua.create_table().expect("table");
        for i in 0..n {
            t.raw_set(format!("k{i}"), i as i32).expect("set");
        }
        Value::Table(t)
    };
    assert!(Compound::from_lua_value(&fill(nbt::MAX_ENTRIES - 1)).is_ok());
    assert!(Compound::from_lua_value(&fill(nbt::MAX_ENTRIES)).is_err());

    // A string of MAX_STRING bytes is read; one more byte is not.
    let string = |n: usize| Value::String(lua.create_string(vec![b'a'; n]).expect("string"));
    let ok = lua.create_table().expect("table");
    ok.raw_set("s", string(nbt::MAX_STRING)).expect("set");
    assert!(Compound::from_lua_value(&Value::Table(ok.clone())).is_ok());
    ok.raw_set("s", string(nbt::MAX_STRING + 1)).expect("set");
    assert!(Compound::from_lua_value(&Value::Table(ok)).is_err());

    assert!(
        start.elapsed() < Duration::from_secs(2),
        "limits took {:?}",
        start.elapsed()
    );
}

#[test]
fn integers_refuse_out_of_range_and_fractional_numbers() {
    let mut rng = Rng::new(SEED_STRICT);
    for case in 0..100 {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                i64::from_lua_value(&Value::Number(value)).is_err(),
                "case {case}"
            );
        }
        macro_rules! refuse {
            ($($ty:ty),+) => {$({
                let big = <$ty>::MAX as f64 + 1.0 + rng.below(1000) as f64;
                // Twice the maximum's magnitude: f64 cannot tell MIN from MIN - 1000 at 2^63.
                let small = -(<$ty>::MAX as f64) * 2.0 - 1.0 - rng.below(1000) as f64;
                let fraction = rng.below(1000) as f64 + 0.25;
                for n in [big, small, fraction] {
                    assert!(
                        <$ty>::from_lua_value(&Value::Number(n)).is_err(),
                        "{} accepted {n} (seed {SEED_STRICT:#x} case {case})",
                        stringify!($ty)
                    );
                }
            })+};
        }
        refuse!(i8, u8, i16, u16, i32, u32, i64, u64, usize);
    }
    assert!(u8::from_lua_value(&Value::Integer(-1)).is_err());
}

/// A defect found by this file, kept as an ignored test. Remove the ignore when it is fixed.
#[test]
fn long_array_keeps_longs_past_the_exact_limit() {
    let lua = Lua::new();
    let tag = Tag::LongArray(vec![(1 << 60) + 1]);
    assert_eq!(round_trip(&lua, &tag), tag);
}
