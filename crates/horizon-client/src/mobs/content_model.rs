//! A content mod's own mob model: a tree of named parts holding cubes, as a
//! vanilla `LayerDefinition` is written (`assets/<ns>/models/entity/<name>.json`),
//! turned into the baked [`Part`] tree the renderer draws, with the face
//! layout vanilla's `ModelPart.Cube` gives a box texture. Parts keep the
//! names an animation set moves (`head`, `body`, `right_arm`...).
//!
//! ```json
//! { "texture_size": [64, 64],
//!   "parts": {
//!     "head": { "offset": [0, 0, 0], "rotation": [0, 0, 0],
//!               "cubes": [{ "origin": [-4, -8, -4], "size": [8, 8, 8], "uv": [0, 0], "inflate": 0, "mirror": false }],
//!               "children": { "hat": { "cubes": [{ "origin": [-4, -8, -4], "size": [8, 8, 8], "uv": [32, 0], "inflate": 0.5 }] } } },
//!     "right_arm": { "offset": [-5, 2, 0], "cubes": [{ "origin": [-3, -2, -2], "size": [4, 12, 4], "uv": [40, 16] }] }
//!   } }
//! ```
//!
//! `offset` is the part's pivot in its parent (vanilla's `PartPose`, y down
//! from the parent's pivot, the root's 24 above the feet), `rotation` in
//! degrees; a cube's `origin` and `size` are in that space, `uv` the box's
//! top-left corner on the texture, `inflate` grows it on every side.
use super::{Anim, Part, Polygon};
use serde_json::Value;

/// The model `v`, as a root part named `root`.
pub fn parse(v: &Value) -> Result<Part, String> {
    let size = v["texture_size"]
        .as_array()
        .map(|a| {
            [
                a.first().and_then(Value::as_f64).unwrap_or(64.0) as f32,
                a.get(1).and_then(Value::as_f64).unwrap_or(64.0) as f32,
            ]
        })
        .unwrap_or([64.0, 64.0]);
    if size[0] <= 0.0 || size[1] <= 0.0 {
        return Err("texture_size: two sizes above 0".into());
    }
    let parts = v["parts"].as_object().ok_or("no parts object")?;
    let mut root = Part {
        name: "root".into(),
        pose: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        polygons: Vec::new(),
        children: Vec::new(),
    };
    for (name, part) in parts {
        root.children.push(parse_part(name, part, size, 0)?);
    }
    Ok(root)
}

fn parse_part(name: &str, v: &Value, size: [f32; 2], depth: usize) -> Result<Part, String> {
    if depth > 16 {
        return Err(format!("{name}: parts nested deeper than 16"));
    }
    let at = |e: String| format!("{name}: {e}");
    let triple = |key: &str, default: f32| -> Result<[f32; 3], String> {
        match v.get(key) {
            None => Ok([default; 3]),
            Some(Value::Array(a)) if a.len() == 3 => {
                let mut out = [default; 3];
                for (i, x) in a.iter().enumerate() {
                    out[i] = x
                        .as_f64()
                        .ok_or_else(|| at(format!("{key}: not three numbers")))?
                        as f32;
                }
                Ok(out)
            }
            Some(_) => Err(at(format!("{key}: not three numbers"))),
        }
    };
    let offset = triple("offset", 0.0)?;
    let rotation = triple("rotation", 0.0)?.map(f32::to_radians);
    let scale = triple("scale", 1.0)?;
    let mut part = Part {
        name: name.to_owned(),
        pose: [
            offset[0],
            offset[1],
            offset[2],
            rotation[0],
            rotation[1],
            rotation[2],
            scale[0],
            scale[1],
            scale[2],
        ],
        polygons: Vec::new(),
        children: Vec::new(),
    };
    for cube in v["cubes"].as_array().into_iter().flatten() {
        let number =
            |key: &str, default: f32| -> f32 { cube[key].as_f64().map_or(default, |x| x as f32) };
        let three = |key: &str| -> Result<[f32; 3], String> {
            let a = cube[key]
                .as_array()
                .filter(|a| a.len() == 3)
                .ok_or_else(|| at(format!("a cube's {key}: not three numbers")))?;
            let mut out = [0.0; 3];
            for (i, x) in a.iter().enumerate() {
                out[i] = x
                    .as_f64()
                    .ok_or_else(|| at(format!("a cube's {key}: not three numbers")))?
                    as f32;
            }
            Ok(out)
        };
        let origin = three("origin")?;
        let dims = three("size")?;
        if dims.iter().any(|d| *d < 0.0) {
            return Err(at("a cube's size is negative".into()));
        }
        let uv = match cube["uv"].as_array() {
            Some(a) if a.len() == 2 => [
                a[0].as_f64().unwrap_or(0.0) as f32,
                a[1].as_f64().unwrap_or(0.0) as f32,
            ],
            _ => [0.0, 0.0],
        };
        let inflate = number("inflate", 0.0);
        let mirror = cube["mirror"].as_bool().unwrap_or(false);
        part.polygons
            .extend(cube_faces(origin, dims, uv, inflate, mirror, size));
    }
    if let Some(children) = v["children"].as_object() {
        for (child, cv) in children {
            part.children.push(parse_part(child, cv, size, depth + 1)?);
        }
    }
    Ok(part)
}

/// The six faces of a box, laid out on the texture as vanilla's
/// `ModelPart.Cube` lays them: the top and bottom over the sides, the sides
/// west, north, east, south in a row.
fn cube_faces(
    origin: [f32; 3],
    dims: [f32; 3],
    uv: [f32; 2],
    inflate: f32,
    mirror: bool,
    size: [f32; 2],
) -> Vec<Polygon> {
    let [dx, dy, dz] = dims;
    let (x0, y0, z0) = (
        origin[0] - inflate,
        origin[1] - inflate,
        origin[2] - inflate,
    );
    let (x1, y1, z1) = (
        origin[0] + dx + inflate,
        origin[1] + dy + inflate,
        origin[2] + dz + inflate,
    );
    let (u, v) = (uv[0], uv[1]);
    let tex = |px: f32, py: f32| [px / size[0], py / size[1]];
    let vert = |x: f32, y: f32, z: f32, px: f32, py: f32| {
        let [tu, tv] = tex(px, py);
        [x, y, z, tu, tv]
    };
    let faces = [
        // top (y0), looking down from above
        (
            [0.0, -1.0, 0.0],
            [
                vert(x1, y0, z1, u + dz + dx, v),
                vert(x0, y0, z1, u + dz, v),
                vert(x0, y0, z0, u + dz, v + dz),
                vert(x1, y0, z0, u + dz + dx, v + dz),
            ],
        ),
        // bottom (y1)
        (
            [0.0, 1.0, 0.0],
            [
                vert(x1, y1, z0, u + dz + dx + dx, v + dz),
                vert(x0, y1, z0, u + dz + dx, v + dz),
                vert(x0, y1, z1, u + dz + dx, v),
                vert(x1, y1, z1, u + dz + dx + dx, v),
            ],
        ),
        // west (x0)
        (
            [-1.0, 0.0, 0.0],
            [
                vert(x0, y0, z0, u + dz, v + dz),
                vert(x0, y0, z1, u, v + dz),
                vert(x0, y1, z1, u, v + dz + dy),
                vert(x0, y1, z0, u + dz, v + dz + dy),
            ],
        ),
        // north (z0)
        (
            [0.0, 0.0, -1.0],
            [
                vert(x1, y0, z0, u + dz + dx, v + dz),
                vert(x0, y0, z0, u + dz, v + dz),
                vert(x0, y1, z0, u + dz, v + dz + dy),
                vert(x1, y1, z0, u + dz + dx, v + dz + dy),
            ],
        ),
        // east (x1)
        (
            [1.0, 0.0, 0.0],
            [
                vert(x1, y0, z1, u + dz + dx + dz, v + dz),
                vert(x1, y0, z0, u + dz + dx, v + dz),
                vert(x1, y1, z0, u + dz + dx, v + dz + dy),
                vert(x1, y1, z1, u + dz + dx + dz, v + dz + dy),
            ],
        ),
        // south (z1)
        (
            [0.0, 0.0, 1.0],
            [
                vert(x0, y0, z1, u + dz + dx + dz + dx, v + dz),
                vert(x1, y0, z1, u + dz + dx + dz, v + dz),
                vert(x1, y1, z1, u + dz + dx + dz, v + dz + dy),
                vert(x0, y1, z1, u + dz + dx + dz + dx, v + dz + dy),
            ],
        ),
    ];
    faces
        .into_iter()
        .map(|(normal, mut vertices)| {
            if mirror {
                // A mirrored cube reads its texture right to left (vanilla
                // swaps the vertices' u in pairs).
                let (a, b) = (vertices[0][3], vertices[1][3]);
                vertices[0][3] = b;
                vertices[1][3] = a;
                let (c, d) = (vertices[2][3], vertices[3][3]);
                vertices[2][3] = d;
                vertices[3][3] = c;
            }
            Polygon {
                normal,
                vertices,
                cull_back: false,
            }
        })
        .collect()
}

/// An animation set by its name in a mod's data (`zombie`, `quadruped`,
/// `iron_golem`...): the enum's variant in snake case.
pub fn anim_by_name(name: &str) -> Option<Anim> {
    ANIMS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
}

/// Every animation set a content mob may ask for, by name.
pub const ANIMS: &[(&str, Anim)] = &[
    ("nautilus", Anim::Nautilus),
    ("happy_ghast", Anim::HappyGhast),
    ("sniffer", Anim::Sniffer),
    ("breeze", Anim::Breeze),
    ("warden", Anim::Warden),
    ("allay", Anim::Allay),
    ("creaking", Anim::Creaking),
    ("camel", Anim::Camel),
    ("wither", Anim::Wither),
    ("vex", Anim::Vex),
    ("ravager", Anim::Ravager),
    ("guardian", Anim::Guardian),
    ("shulker", Anim::Shulker),
    ("quadruped", Anim::Quadruped),
    ("sheep", Anim::Sheep),
    ("chicken", Anim::Chicken),
    ("zombie", Anim::Zombie),
    ("drowned", Anim::Drowned),
    ("zombie_villager", Anim::ZombieVillager),
    ("skeleton", Anim::Skeleton),
    ("creeper", Anim::Creeper),
    ("spider", Anim::Spider),
    ("enderman", Anim::Enderman),
    ("silverfish", Anim::Silverfish),
    ("slime", Anim::Slime),
    ("magma_cube", Anim::MagmaCube),
    ("witch", Anim::Witch),
    ("villager", Anim::Villager),
    ("iron_golem", Anim::IronGolem),
    ("snow_golem", Anim::SnowGolem),
    ("copper_golem", Anim::CopperGolem),
    ("illager", Anim::Illager),
    ("piglin", Anim::Piglin),
    ("zombified_piglin", Anim::ZombifiedPiglin),
    ("hoglin", Anim::Hoglin),
    ("wolf", Anim::Wolf),
    ("feline", Anim::Feline),
    ("fox", Anim::Fox),
    ("rabbit", Anim::Rabbit),
    ("horse", Anim::Horse),
    ("llama", Anim::Llama),
    ("goat", Anim::Goat),
    ("panda", Anim::Panda),
    ("polar_bear", Anim::PolarBear),
    ("parrot", Anim::Parrot),
    ("bee", Anim::Bee),
    ("turtle", Anim::Turtle),
    ("armadillo", Anim::Armadillo),
    ("frog", Anim::Frog),
    ("axolotl", Anim::Axolotl),
    ("squid", Anim::Squid),
    ("fish", Anim::Fish),
    ("pufferfish", Anim::Pufferfish),
    ("dolphin", Anim::Dolphin),
    ("bat", Anim::Bat),
    ("phantom", Anim::Phantom),
    ("ghast", Anim::Ghast),
    ("blaze", Anim::Blaze),
    ("strider", Anim::Strider),
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_cube_gets_the_six_faces_vanilla_lays_out() {
        // The zombie's head, as `entity-models.json` exports it.
        let model = json!({
            "texture_size": [64, 64],
            "parts": {
                "head": {
                    "cubes": [{"origin": [-4, -8, -4], "size": [8, 8, 8], "uv": [0, 0]}],
                    "children": {"hat": {"cubes": [{"origin": [-4, -8, -4], "size": [8, 8, 8], "uv": [32, 0], "inflate": 0.5}]}}
                },
                "right_arm": {"offset": [-5, 2, 0], "rotation": [0, 0, 90], "cubes": []}
            }
        });
        let root = parse(&model).unwrap();
        assert_eq!(root.children.len(), 2);
        let head = &root.children[0];
        assert_eq!(head.name, "head");
        assert_eq!(head.polygons.len(), 6);
        let faces: Vec<(&[f32; 3], &[[f32; 5]; 4])> = head
            .polygons
            .iter()
            .map(|p| (&p.normal, &p.vertices))
            .collect();
        assert_eq!(
            faces[0],
            (
                &[0.0, -1.0, 0.0],
                &[
                    [4.0, -8.0, 4.0, 0.25, 0.0],
                    [-4.0, -8.0, 4.0, 0.125, 0.0],
                    [-4.0, -8.0, -4.0, 0.125, 0.125],
                    [4.0, -8.0, -4.0, 0.25, 0.125]
                ]
            )
        );
        assert_eq!(
            faces[5],
            (
                &[0.0, 0.0, 1.0],
                &[
                    [-4.0, -8.0, 4.0, 0.5, 0.125],
                    [4.0, -8.0, 4.0, 0.375, 0.125],
                    [4.0, 0.0, 4.0, 0.375, 0.25],
                    [-4.0, 0.0, 4.0, 0.5, 0.25]
                ]
            )
        );
        // The hat is inflated half a block and sits under the head.
        let hat = &head.children[0];
        assert_eq!(hat.polygons[0].vertices[0][..3], [4.5, -8.5, 4.5]);
        assert_eq!(hat.polygons[0].vertices[0][3], 0.75);
        // A part's pose: offset, rotation in radians, scale 1.
        let arm = &root.children[1];
        assert_eq!(arm.pose[..3], [-5.0, 2.0, 0.0]);
        assert!((arm.pose[5] - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(arm.pose[6..], [1.0, 1.0, 1.0]);
        assert_eq!(anim_by_name("zombie"), Some(Anim::Zombie));
        assert_eq!(anim_by_name("iron_golem"), Some(Anim::IronGolem));
        assert_eq!(anim_by_name("dragon"), None);
        assert!(
            parse(&json!({"parts": {"a": {"cubes": [{"origin": [0, 0], "size": [1, 1, 1]}]}}}))
                .is_err()
        );
    }
}
