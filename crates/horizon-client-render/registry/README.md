# Feature Registry

This folder holds one `<id>.json` file for each switchable feature that this build carries: the built-in native
hook mods, such as zoom and the minimap, and the engine patches, such as sharpening, anti-lag and mesh shaders.

`build.rs` collects the files, and `src/mod_registry.rs` reads them. The game uses them in three places:

- Options > Mods lists the features.
- `horizon-client --list-mods` prints them as JSON, for the launcher.
- `horizon-mods.txt` keeps a `mod.<id>` line for each feature, in step with the setting the feature switches. The
  launcher turns a feature on or off by writing that line.

A feature branch adds its own file and nothing else, so two branches never edit the same file.

This file declares AMD's contrast adaptive sharpening:

```json
{
  "id": "cas",
  "title": "AMD CAS 1.0",
  "description": "Contrast adaptive sharpening after TAA and resolution scaling.",
  "kind": "engine",
  "default_on": false,
  "setting": { "file": "graphics", "key": "cas", "on": "always", "off": "off" }
}
```

| Field | Description |
|---|---|
| `id` | the feature's id, used in its `mod.<id>` line |
| `title` | the name shown to players, with its version |
| `description` | one sentence that says what the feature does |
| `kind` | `hook` for a built-in native hook mod, or `engine` for an engine patch |
| `default_on` | true if a fresh game folder has the feature on, before anything is written |
| `setting.file` | the settings file the feature's switch lives in: `graphics` for `horizon-graphics.txt`, or `extras` for `horizon-extras.txt` |
| `setting.key` | the key of the switch's line in that file |
| `setting.on`, `setting.off` | the values written to turn the feature on and off |

The feature counts as on whenever its key's value is anything other than the `off` value, so a mode such as
`always` counts as on. Turning a feature on leaves a mode that is already chosen as it is.
