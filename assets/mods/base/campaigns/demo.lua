-- Demo campaign: a light narrative wrapper around the frontier sandbox.
--
-- API summary (all side indices are 0-based, matching the map file):
--   game.message(text)             show a message in the campaign log
--   game.set_funds(side, amount)   overwrite a side's funds
--   game.give_funds(side, amount)  add to a side's funds
--   game.start_battle(map_id)      launch a scenario battle map
--
-- Hooks receive a ctx table: ctx.turn, ctx.funds[side],
-- ctx.army_counts[side], ctx.side_names[side].

campaign = {}

function campaign.on_start(ctx)
    game.message("Operation Frontier begins.")
    game.message("Seize the factories before the Iron Valkyries dig in.")
    game.give_funds(0, 10)
end

function campaign.on_turn(ctx)
    if ctx.turn == 3 then
        game.message("Day 3: the academy wires additional funding.")
        game.give_funds(0, 15)
    end
    if ctx.turn == 6 and ctx.army_counts[1] > 0 then
        game.message("Day 6: scouts report Valkyrie supply problems.")
        game.give_funds(1, -10)
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
