//! A tag as SNBT, for the wire.

use horizon_world::nbt::Tag;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub fn serialize<S: Serializer>(tag: &Option<Tag>, s: S) -> Result<S::Ok, S::Error> {
    tag.as_ref()
        .map(horizon_world::snbt_text::to_snbt)
        .serialize(s)
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Tag>, D::Error> {
    Ok(Option::<String>::deserialize(d)?
        .and_then(|t| horizon_world::nbt::parse_snbt(&t).ok().map(|(tag, _)| tag)))
}
