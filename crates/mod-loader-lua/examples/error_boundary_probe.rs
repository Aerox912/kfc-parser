fn main() -> mlua::Result<()> {
    let lua = mlua::Lua::new();
    lua.globals().set(
        "expected_error",
        lua.create_function(|_, ()| -> mlua::Result<()> {
            Err(mlua::Error::external("expected callback error"))
        })?,
    )?;
    let (ok, message): (bool, String) = lua
        .load("local ok, err = pcall(expected_error); return ok, tostring(err)")
        .eval()?;
    assert!(!ok, "the callback must fail");
    assert!(message.contains("expected callback error"), "{message}");
    println!("PASS: Lua pcall catches a Rust callback error without aborting");
    Ok(())
}
