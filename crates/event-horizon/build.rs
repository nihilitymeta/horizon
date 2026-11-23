//! Gives the Windows executable Horizon's icon and its stack, as horizon-client's
//! (`packaging/windows/exe.rs`), and writes the Lua host's package beside the executable, where
//! the loader finds the built-in mod.
use std::env;
use std::path::PathBuf;

#[path = "../../packaging/windows/exe.rs"]
mod windows_exe;

#[allow(dead_code)]
#[path = "../event-horizon-lua/src/package.rs"]
mod lua_package;

#[allow(dead_code)]
#[path = "../event-horizon-dev-tools/src/package.rs"]
mod dev_tools_package;

fn main() {
    windows_exe::resources("event-horizon");
    println!("cargo:rerun-if-changed=../event-horizon-lua/src/package.rs");
    println!("cargo:rerun-if-changed=../event-horizon-dev-tools/src/package.rs");
    // OUT_DIR is `<target>/<profile>/build/<package>-<hash>/out`, or with `--target` under
    // `<target>/<triple>/`; the executable goes in the profile folder, four levels up.
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let profile = out
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has a profile folder above it");
    let dir = profile.join("horizon-native-mods").join(lua_package::ID);
    lua_package::write_to(&dir).expect("the Lua host's package can be written");
    let dev_dir = profile
        .join("horizon-native-mods")
        .join(dev_tools_package::ID);
    dev_tools_package::write_to(&dev_dir).expect("the developer tools' package can be written");
}
