# Feature registry

One `<id>.json` per toggleable Horizon feature this build carries: the built-in native hook mods
(zoom, minimap…) and the engine patches (sharpening, anti-lag, mesh shaders…). `build.rs` collects
the files; `src/mod_registry.rs` reads them. The game lists them in Options > Mods, prints them with
`horizon-client --list-mods` for the launcher, and keeps their `mod.<id>` lines in `horizon-mods.txt`
in step with the setting each one toggles, so the launcher turns a feature on or off by writing that
line. A feature branch adds its own file and nothing else; two branches never edit the same file.

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

- `kind`: `hook` (a built-in native hook mod) or `engine` (an engine patch).
- `setting.file`: `graphics` (`horizon-graphics.txt`) or `extras` (`horizon-extras.txt`); `key` is the
  line's key there. The feature is on whenever the key's value differs from `off`; turning it on writes
  `on`, turning it off writes `off`.
- `default_on`: what a fresh game folder gets before anything is written.
