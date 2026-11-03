//! NBT as Lua values and back, for the data a script reads and writes:
//! a block entity's saved compound, an entity's data. A compound is a
//! table, a list a sequence, numbers are numbers (a Lua number has no
//! NBT type, so writing back keeps the type the old value had: a float
//! stays a float, a byte a byte), strings are strings. A Luau number is a
//! double: a long past 2^53 reads rounded; one the script leaves as it read
//! it is written back exactly, a changed one rounded, with a warning.
use horizon_world::nbt::{Compound, JavaString, Tag};
use mlua::{Lua, Table, Value};

/// A tag as a Lua value.
pub fn tag_to_lua(lua: &Lua, tag: &Tag) -> mlua::Result<Value> {
    Ok(match tag {
        Tag::End => Value::Nil,
        Tag::Byte(v) => Value::Integer(i64::from(*v)),
        Tag::Short(v) => Value::Integer(i64::from(*v)),
        Tag::Int(v) => Value::Integer(i64::from(*v)),
        Tag::Long(v) => Value::Integer(*v),
        Tag::Float(bits) => Value::Number(f64::from(f32::from_bits(*bits))),
        Tag::Double(bits) => Value::Number(f64::from_bits(*bits)),
        Tag::String(s) => Value::String(lua.create_string(s.to_string_lossy())?),
        Tag::ByteArray(v) => {
            Value::Table(lua.create_sequence_from(v.iter().map(|b| i64::from(*b as i8)))?)
        }
        Tag::IntArray(v) => Value::Table(lua.create_sequence_from(v.iter().copied())?),
        Tag::LongArray(v) => Value::Table(lua.create_sequence_from(v.iter().copied())?),
        Tag::List(items) => {
            let t = lua.create_table()?;
            for (i, item) in items.iter().enumerate() {
                t.set(i + 1, tag_to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        Tag::Compound(c) => Value::Table(compound_to_table(lua, c)?),
    })
}

/// A compound as a table.
pub fn compound_to_table(lua: &Lua, c: &Compound) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    for (k, v) in c {
        t.set(k.to_string_lossy(), tag_to_lua(lua, v)?)?;
    }
    Ok(t)
}

/// How deep a saved table may nest: a table in a table in a table... stops
/// here (NBT's own readers stop at 512), with the path to it in the error.
pub const MAX_DEPTH: usize = 128;
/// The largest whole number a Lua number (a double) holds exactly: 2^53.
pub const EXACT: f64 = 9_007_199_254_740_992.0;

/// The walk down a table being saved: the keys from the root to the value
/// in hand (an error names them) and the tables on that path, so a table
/// that holds itself, directly or further down, is an error and not a
/// stack that never ends.
#[derive(Default)]
struct Walk {
    path: Vec<String>,
    tables: Vec<*const std::ffi::c_void>,
}
impl Walk {
    /// The value in hand, for an error: `items[2].count`, or `the table`.
    fn at(&self) -> String {
        if self.path.is_empty() {
            "the table".to_owned()
        } else {
            self.path.concat()
        }
    }
    fn enter(&mut self, t: &Table) -> mlua::Result<()> {
        let p = t.to_pointer();
        if self.tables.contains(&p) {
            return Err(mlua::Error::runtime(format!(
                "{}: a table that holds itself (a cycle) cannot be saved as NBT",
                self.at()
            )));
        }
        if self.tables.len() >= MAX_DEPTH {
            return Err(mlua::Error::runtime(format!(
                "{}: nested deeper than {MAX_DEPTH} tables",
                self.at()
            )));
        }
        self.tables.push(p);
        Ok(())
    }
    fn leave(&mut self) {
        self.tables.pop();
    }
    /// `f` with `segment` on the path.
    fn under<R>(
        &mut self,
        segment: String,
        f: impl FnOnce(&mut Self) -> mlua::Result<R>,
    ) -> mlua::Result<R> {
        self.path.push(segment);
        let r = f(self);
        self.path.pop();
        r
    }
}

/// A Lua value as a tag. `like` is the tag it replaces, if any: its type is
/// kept for a number (a float stays a float), its element type for a list.
/// Without one, an integer is an int (a long past 32 bits), another number
/// a double, a boolean a byte, a sequence a list and another table a compound.
/// A table nested past [`MAX_DEPTH`] or holding itself is an error naming
/// the key path.
pub fn lua_to_tag(v: &Value, like: Option<&Tag>) -> mlua::Result<Tag> {
    Walk::default().tag(v, like)
}

impl Walk {
    fn tag(&mut self, v: &Value, like: Option<&Tag>) -> mlua::Result<Tag> {
        let number = |n: f64| -> Tag {
            match like {
                Some(Tag::Byte(_)) => Tag::Byte(n as i8),
                Some(Tag::Short(_)) => Tag::Short(n as i16),
                Some(Tag::Int(_)) => Tag::Int(n as i32),
                // The number the script got for the old long is the old
                // long rounded to a double: the same number back means the
                // script left it alone, and the exact value is kept.
                Some(Tag::Long(old)) if *old as f64 == n => Tag::Long(*old),
                Some(Tag::Long(_)) => {
                    if n.abs() > EXACT {
                        log::warn!(
                            "[lua] {}: {n} is past 2^53, the largest whole number a Lua number holds exactly: saved rounded",
                            self.at()
                        );
                    }
                    Tag::Long(n as i64)
                }
                Some(Tag::Float(_)) => Tag::Float((n as f32).to_bits()),
                Some(Tag::Double(_)) => Tag::Double(n.to_bits()),
                _ if n.fract() == 0.0 && n.abs() < 2f64.powi(31) => Tag::Int(n as i32),
                _ if n.fract() == 0.0 => Tag::Long(n as i64),
                _ => Tag::Double(n.to_bits()),
            }
        };
        Ok(match v {
            Value::Nil => Tag::End,
            Value::Boolean(b) => match like {
                Some(Tag::Int(_)) => Tag::Int(i32::from(*b)),
                _ => Tag::Byte(i8::from(*b)),
            },
            Value::Integer(i) => match like {
                Some(Tag::Float(_)) | Some(Tag::Double(_)) => number(*i as f64),
                Some(Tag::Byte(_)) => Tag::Byte(*i as i8),
                Some(Tag::Short(_)) => Tag::Short(*i as i16),
                // A whole double comes back as an integer: the same number as
                // the old long rounded means the script left it alone.
                Some(Tag::Long(_)) => number(*i as f64),
                Some(Tag::Int(_)) => Tag::Int(*i as i32),
                _ if i32::try_from(*i).is_ok() => Tag::Int(*i as i32),
                _ => Tag::Long(*i),
            },
            Value::Number(n) => number(*n),
            Value::String(s) => Tag::String(JavaString::from(s.to_string_lossy().as_ref())),
            Value::Table(t) => {
                self.enter(t)?;
                let tag = self.table(t, like)?;
                self.leave();
                tag
            }
            other => {
                return Err(mlua::Error::runtime(format!(
                    "{}: a {} cannot be saved as NBT",
                    self.at(),
                    other.type_name()
                )));
            }
        })
    }

    /// A table already entered as a list, an array or a compound.
    fn table(&mut self, t: &Table, like: Option<&Tag>) -> mlua::Result<Tag> {
        let len = t.raw_len();
        let sequence = len > 0
            || matches!(
                like,
                Some(Tag::List(_) | Tag::IntArray(_) | Tag::LongArray(_) | Tag::ByteArray(_))
            );
        if !sequence {
            let old = match like {
                Some(Tag::Compound(c)) => Some(c),
                _ => None,
            };
            return Ok(Tag::Compound(self.compound(t, old)?));
        }
        // A list has no names: a key beside the sequence would be lost, so
        // it is an error naming the key instead.
        for pair in t.clone().pairs::<Value, Value>() {
            let (k, _) = pair?;
            let in_sequence = matches!(k, Value::Integer(i) if i >= 1 && i as usize <= len);
            if !in_sequence {
                let key = match &k {
                    Value::String(s) => s.to_string_lossy(),
                    Value::Integer(i) => i.to_string(),
                    Value::Number(n) => n.to_string(),
                    other => other.type_name().to_owned(),
                };
                return Err(mlua::Error::runtime(format!(
                    "{}: a table with the items 1..{len} is saved as a list, which has no room for \
                     the key {key}; keep the named values in a table of their own",
                    self.at()
                )));
            }
        }
        Ok(match like {
            Some(Tag::IntArray(_)) => {
                let mut out = Vec::with_capacity(len);
                for i in 1..=len {
                    out.push(t.raw_get::<i64>(i)? as i32);
                }
                Tag::IntArray(out)
            }
            Some(Tag::LongArray(_)) => {
                let mut out = Vec::with_capacity(len);
                for i in 1..=len {
                    out.push(t.raw_get::<i64>(i)?);
                }
                Tag::LongArray(out)
            }
            Some(Tag::ByteArray(_)) => {
                let mut out = Vec::with_capacity(len);
                for i in 1..=len {
                    out.push(t.raw_get::<i64>(i)? as i8 as u8);
                }
                Tag::ByteArray(out)
            }
            _ => {
                let old = match like {
                    Some(Tag::List(items)) => items.first(),
                    _ => None,
                };
                let mut out = Vec::with_capacity(len);
                for i in 1..=len {
                    let item: Value = t.raw_get(i)?;
                    out.push(self.under(format!("[{i}]"), |w| w.tag(&item, old))?);
                }
                Tag::List(out)
            }
        })
    }

    /// A table already entered as a compound.
    fn compound(&mut self, t: &Table, like: Option<&Compound>) -> mlua::Result<Compound> {
        let mut out = Compound::new();
        for pair in t.clone().pairs::<Value, Value>() {
            let (k, v) = pair?;
            let key = match k {
                Value::String(s) => s.to_string_lossy(),
                Value::Integer(i) => i.to_string(),
                Value::Number(n) => n.to_string(),
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "{}: a {} is not an NBT key",
                        self.at(),
                        other.type_name()
                    )));
                }
            };
            let key = JavaString::from(key.as_str());
            let old = like.and_then(|c| c.get(&key));
            let segment = if self.path.is_empty() {
                key.to_string_lossy()
            } else {
                format!(".{}", key.to_string_lossy())
            };
            let tag = self.under(segment, |w| w.tag(&v, old))?;
            if tag != Tag::End {
                out.insert(key, tag);
            }
        }
        Ok(out)
    }
}

/// A table as a compound; `like` is the compound it replaces, whose value
/// types are kept key by key. A key set to nil is gone. A table nested past
/// [`MAX_DEPTH`] or holding itself is an error naming the key path.
pub fn table_to_compound(t: &Table, like: Option<&Compound>) -> mlua::Result<Compound> {
    let mut walk = Walk::default();
    walk.enter(t)?;
    walk.compound(t, like)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compound_goes_to_lua_and_back_with_its_types_kept() {
        let lua = Lua::new();
        let mut c = Compound::new();
        c.insert("charge".into(), Tag::Int(3));
        c.insert("heat".into(), Tag::Float(1.5f32.to_bits()));
        c.insert("name".into(), Tag::String("hive".into()));
        c.insert("flag".into(), Tag::Byte(1));
        c.insert("ids".into(), Tag::IntArray(vec![1, 2]));
        c.insert("list".into(), Tag::List(vec![Tag::Short(5), Tag::Short(6)]));
        let mut inner = Compound::new();
        inner.insert("x".into(), Tag::Long(1 << 40));
        c.insert("inner".into(), Tag::Compound(inner));
        let t = compound_to_table(&lua, &c).unwrap();
        lua.globals().set("d", &t).unwrap();
        lua.load(
            "d.charge = d.charge + 1; d.heat = d.heat * 2; d.flag = false; d.ids[3] = 9; \
             d.list[1] = 7; d.inner.x = d.inner.x + 1; d.gone = nil; d.new = 2.5; d.count = 4; \
             d.big = 5000000000; d.tags = {'a', 'b'}; d.sub = {k = 'v'}",
        )
        .exec()
        .unwrap();
        let back = table_to_compound(&t, Some(&c)).unwrap();
        assert_eq!(back.get(&"charge".into()), Some(&Tag::Int(4)));
        assert_eq!(
            back.get(&"heat".into()),
            Some(&Tag::Float(3.0f32.to_bits()))
        );
        assert_eq!(back.get(&"flag".into()), Some(&Tag::Byte(0)));
        assert_eq!(back.get(&"ids".into()), Some(&Tag::IntArray(vec![1, 2, 9])));
        assert_eq!(
            back.get(&"list".into()),
            Some(&Tag::List(vec![Tag::Short(7), Tag::Short(6)]))
        );
        assert_eq!(back.get(&"name".into()), Some(&Tag::String("hive".into())));
        let Some(Tag::Compound(inner)) = back.get(&"inner".into()) else {
            panic!()
        };
        assert_eq!(inner.get(&"x".into()), Some(&Tag::Long((1 << 40) + 1)));
        // New keys get the natural types; a key set to nil is gone.
        assert_eq!(
            back.get(&"new".into()),
            Some(&Tag::Double(2.5f64.to_bits()))
        );
        assert_eq!(back.get(&"count".into()), Some(&Tag::Int(4)));
        assert_eq!(back.get(&"big".into()), Some(&Tag::Long(5_000_000_000)));
        assert_eq!(
            back.get(&"tags".into()),
            Some(&Tag::List(vec![
                Tag::String("a".into()),
                Tag::String("b".into())
            ]))
        );
        assert!(matches!(back.get(&"sub".into()), Some(Tag::Compound(_))));
        assert!(!back.contains_key(&"gone".into()));
        // Unchanged data reads back equal.
        let same = table_to_compound(&compound_to_table(&lua, &c).unwrap(), Some(&c)).unwrap();
        assert_eq!(same, c);
    }

    #[test]
    fn a_cycle_or_a_deep_nest_is_an_error_naming_the_path() {
        let lua = Lua::new();
        let t: Table = lua
            .load("local d = { items = { { count = 1 } } }; d.items[1].me = d; return d")
            .eval()
            .unwrap();
        let e = table_to_compound(&t, None).unwrap_err().to_string();
        assert!(
            e.contains("items[1].me") && e.contains("holds itself"),
            "{e}"
        );
        let t: Table = lua
            .load("local t = {} for i = 1, 1000 do t = { t } end return { deep = t }")
            .eval()
            .unwrap();
        let e = table_to_compound(&t, None).unwrap_err().to_string();
        assert!(e.contains("deeper than") && e.contains("deep[1][1]"), "{e}");
        // A table reached twice, not on its own path, is saved twice.
        let t: Table = lua
            .load("local shared = { x = 1 } return { a = shared, b = { shared } }")
            .eval()
            .unwrap();
        let c = table_to_compound(&t, None).unwrap();
        let Some(Tag::List(b)) = c.get(&"b".into()) else {
            panic!("{c:?}");
        };
        assert_eq!(c.get(&"a".into()), b.first());
    }

    #[test]
    fn a_long_past_2_53_survives_a_round_trip_when_left_alone() {
        let lua = Lua::new();
        let big = (1i64 << 60) + 1;
        let mut c = Compound::new();
        c.insert("big".into(), Tag::Long(big));
        c.insert("other".into(), Tag::Long(big));
        c.insert("changed".into(), Tag::Long(big));
        let t = compound_to_table(&lua, &c).unwrap();
        lua.globals().set("d", &t).unwrap();
        // `other` is set to the same (rounded) number: unchanged as far as a
        // double can tell, so the exact long stays; `changed` is a new value
        // a double holds exactly.
        lua.load("d.other = d.other + 0; d.changed = 3 * 2^53")
            .exec()
            .unwrap();
        let back = table_to_compound(&t, Some(&c)).unwrap();
        assert_eq!(back.get(&"big".into()), Some(&Tag::Long(big)));
        assert_eq!(back.get(&"other".into()), Some(&Tag::Long(big)));
        assert_eq!(back.get(&"changed".into()), Some(&Tag::Long(3 << 53)));
    }

    #[test]
    fn a_sequence_with_a_named_key_beside_it_is_refused_not_cut() {
        let lua = Lua::new();
        let t: Table = lua
            .load("local d = { items = { 'a' } } d.items.count = 1 return d")
            .eval()
            .unwrap();
        let e = table_to_compound(&t, None).unwrap_err().to_string();
        assert!(
            e.contains("items") && e.contains("count") && e.contains("list"),
            "{e}"
        );
        // The same when the old value was a list and the new table is empty
        // but for its names.
        let mut c = Compound::new();
        c.insert("items".into(), Tag::List(vec![Tag::String("a".into())]));
        let t: Table = lua.load("return { items = { count = 1 } }").eval().unwrap();
        let e = table_to_compound(&t, Some(&c)).unwrap_err().to_string();
        assert!(e.contains("count"), "{e}");
        // A plain sequence, or a plain compound, is as before.
        let t: Table = lua
            .load("return { items = { 'a', 'b' }, named = { count = 1 } }")
            .eval()
            .unwrap();
        let c = table_to_compound(&t, None).unwrap();
        assert!(matches!(c.get(&"items".into()), Some(Tag::List(l)) if l.len() == 2));
        assert!(matches!(c.get(&"named".into()), Some(Tag::Compound(_))));
    }
}
