//! Run with `cargo run --release -p neoism-lua --features runtime --example release_smoke`.
//! This must be an executable: Cargo's test harness uses unwinding even when
//! the release profile specifies panic=abort, masking Windows Lua/SEH failures.

use mlua::{Error, HookTriggers, Lua};

fn main() -> mlua::Result<()> {
    let lua = Lua::new();
    lua.globals().set(
        "fail",
        lua.create_function(|_, ()| {
            Err::<(), _>(Error::runtime("expected callback error"))
        })?,
    )?;

    // An ordinary Rust-backed Lua callback error must reach pcall, not abort
    // while Windows unwinds the callback's C-unwind frame.
    let error = lua.load("fail()").exec().expect_err("callback must fail");
    assert!(error.to_string().contains("expected callback error"));
    assert!(lua
        .load("local ok = pcall(fail); return not ok")
        .eval::<bool>()?);

    // Plugin execution budgets also report errors through a Rust callback.
    lua.set_hook(HookTriggers::new().every_nth_instruction(100), |_, _| {
        Err(Error::runtime("expected budget error"))
    })?;
    let error = lua
        .load("while true do end")
        .exec()
        .expect_err("hook must stop execution");
    lua.remove_hook();
    assert!(error.to_string().contains("expected budget error"));

    assert_eq!(lua.load("return 6 * 7").eval::<i32>()?, 42);
    println!(
        "Lua release smoke passed: callback and budget errors recover; VM remains usable"
    );
    Ok(())
}
