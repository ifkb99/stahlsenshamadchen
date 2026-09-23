-- Demo campaign: a light narrative wrapper around the frontier sandbox.
--
-- API summary (all side indices are 0-based, matching the map file):
--   game.message(text)             show a message in the campaign log
--   game.start_battle(map_id)      launch a scenario battle map
--
-- Hooks receive a ctx table: ctx.turn, ctx.army_counts[side],
-- ctx.side_names[side].
--
-- PARKED (2026-09-23): only loaded when the game is built with
-- `--features lua-campaigns`. See PARKED.md.

campaign = {}

function campaign.on_start(ctx)
    game.message("Operation Frontier begins.")
    game.message("Seize the factories before the Iron Valkyries dig in.")
end

function campaign.on_turn(ctx)
    if ctx.turn == 6 and ctx.army_counts[1] > 0 then
        game.message("Day 6: scouts report Valkyrie supply problems.")
    end
end

function campaign.on_battle_end(ctx, winner)
    if winner == 0 then
        game.message("A fine victory. The academy's reputation grows.")
    elseif winner == 1 then
        game.message("A hard lesson. Regroup and strike again.")
    else
        game.message("Both sides limp away from the field.")
    end
end
