//! Lua campaign host (via `mlua`, vendored Lua 5.4).
//!
//! A campaign is a Lua script in `mods/<mod>/campaigns/` defining a global
//! `campaign` table of hooks:
//!
//! ```lua
//! function campaign.on_start(ctx) ... end
//! function campaign.on_turn(ctx) ... end
//! function campaign.on_battle_end(ctx, winner) ... end
//! ```
//!
//! Hooks receive a read-only `ctx` snapshot (turn, funds, army counts) and
//! act through the curated `game.*` API, which queues [`CampaignCommand`]s
//! that the engine applies afterwards. Scripts never touch engine internals,
//! so mods stay stable across engine refactors.

use bevy::prelude::*;
use mlua::{Function, Lua, Table};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use tactics_core::overworld::OverworldState;

/// Everything a campaign script may ask the engine to do.
#[derive(Debug, Clone)]
pub enum CampaignCommand {
    Message(String),
    SetFunds { side: u8, amount: i32 },
    GiveFunds { side: u8, amount: i32 },
    StartBattle { map_id: String },
}

/// Non-send resource holding the Lua state (Lua is not thread-safe).
pub struct Campaign {
    lua: Lua,
    queue: Rc<RefCell<Vec<CampaignCommand>>>,
    path: PathBuf,
}

pub struct CampaignPlugin;

impl Plugin for CampaignPlugin {
    fn build(&self, app: &mut App) {
        if let Some(script) = find_campaign_script() {
            match Campaign::load(&script) {
                Ok(campaign) => {
                    info!("loaded campaign script {}", script.display());
                    app.insert_non_send(campaign);
                }
                Err(err) => {
                    error!("campaign script {} failed: {err}", script.display());
                }
            }
        } else {
            info!("no campaign script found; running sandbox mode");
        }
    }
}

fn find_campaign_script() -> Option<PathBuf> {
    let mut scripts: Vec<PathBuf> = std::fs::read_dir("assets/mods")
        .ok()?
        .flatten()
        .filter_map(|m| std::fs::read_dir(m.path().join("campaigns")).ok())
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lua"))
        .collect();
    scripts.sort();
    scripts.into_iter().next()
}

impl Campaign {
    pub fn load(path: &PathBuf) -> mlua::Result<Self> {
        let lua = Lua::new();
        let queue: Rc<RefCell<Vec<CampaignCommand>>> = Rc::default();

        let game = lua.create_table()?;
        let q = queue.clone();
        game.set(
            "message",
            lua.create_function(move |_, text: String| {
                q.borrow_mut().push(CampaignCommand::Message(text));
                Ok(())
            })?,
        )?;
        let q = queue.clone();
        game.set(
            "set_funds",
            lua.create_function(move |_, (side, amount): (u8, i32)| {
                q.borrow_mut().push(CampaignCommand::SetFunds { side, amount });
                Ok(())
            })?,
        )?;
        let q = queue.clone();
        game.set(
            "give_funds",
            lua.create_function(move |_, (side, amount): (u8, i32)| {
                q.borrow_mut().push(CampaignCommand::GiveFunds { side, amount });
                Ok(())
            })?,
        )?;
        let q = queue.clone();
        game.set(
            "start_battle",
            lua.create_function(move |_, map_id: String| {
                q.borrow_mut().push(CampaignCommand::StartBattle { map_id });
                Ok(())
            })?,
        )?;
        lua.globals().set("game", game)?;
        // Ensure the campaign table exists even if the script forgets.
        lua.globals().set("campaign", lua.create_table()?)?;

        let source = std::fs::read_to_string(path).map_err(mlua::Error::external)?;
        lua.load(&source).set_name(path.display().to_string()).exec()?;

        Ok(Self {
            lua,
            queue,
            path: path.clone(),
        })
    }

    pub fn drain_commands(&self) -> Vec<CampaignCommand> {
        self.queue.borrow_mut().drain(..).collect()
    }

    /// Snapshot the overworld into a Lua-friendly ctx table.
    fn context(&self, state: &OverworldState) -> mlua::Result<Table> {
        let ctx = self.lua.create_table()?;
        ctx.set("turn", state.turn)?;
        let funds = self.lua.create_table()?;
        let armies = self.lua.create_table()?;
        let names = self.lua.create_table()?;
        for (i, side) in state.sides.iter().enumerate() {
            funds.set(i, side.funds)?;
            names.set(i, side.name.clone())?;
            armies.set(i, state.side_armies(i as u8).count())?;
        }
        ctx.set("funds", funds)?;
        ctx.set("side_names", names)?;
        ctx.set("army_counts", armies)?;
        Ok(ctx)
    }

    fn call_hook(&self, name: &str, args: impl mlua::IntoLuaMulti) {
        let result: mlua::Result<()> = (|| {
            let campaign: Table = self.lua.globals().get("campaign")?;
            let Ok(hook) = campaign.get::<Function>(name) else {
                return Ok(());
            };
            hook.call::<()>(args)
        })();
        if let Err(err) = result {
            warn!("campaign hook `{name}` ({}) failed: {err}", self.path.display());
        }
    }
}

pub fn call_start_hook(campaign: &Campaign, state: &OverworldState) {
    match campaign.context(state) {
        Ok(ctx) => campaign.call_hook("on_start", ctx),
        Err(err) => warn!("campaign context failed: {err}"),
    }
}

pub fn call_turn_hook(campaign: &Campaign, state: &OverworldState) {
    match campaign.context(state) {
        Ok(ctx) => campaign.call_hook("on_turn", ctx),
        Err(err) => warn!("campaign context failed: {err}"),
    }
}

pub fn call_battle_end_hook(campaign: &Campaign, state: &OverworldState, winner: Option<u8>) {
    match campaign.context(state) {
        Ok(ctx) => campaign.call_hook("on_battle_end", (ctx, winner)),
        Err(err) => warn!("campaign context failed: {err}"),
    }
}
