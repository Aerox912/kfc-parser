//! Execute a local Lua assertion script with the same vendored Lua as EML.
fn main() -> mlua::Result<()> {
    let path = std::env::args().nth(1).expect("usage: lua_contract_probe SCRIPT");
    let source = std::fs::read_to_string(&path).map_err(mlua::Error::external)?;
    mlua::Lua::new().load(&source).set_name(&path).exec()
}
