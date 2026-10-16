"""The reinforcement-learning loop: rollouts in Rust, updates in JAX, repeat.

    uv run loop.py --start ../target/goal_net_places16.json --iterations 40

Each iteration plays `--games` fresh battles (new seeds every iteration) with
the current network *sampling* its choices, takes one PPO update, and every
`--eval-every` iterations measures the network *deterministically* against the
utility AI with the project's own instrument — `balance --brains` on the same
arena, at difficulty 5, over four seeds the rollouts never use — so the number
that says whether it learned anything is the number every other AI change in
this project has been judged by.
"""

from __future__ import annotations

import argparse
import csv
import os
import pathlib
import re
import subprocess
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
EVAL_SEEDS = [100000, 101000, 102000, 103000]


def cargo(example, *args):
    # `STAHL_BIN_DIR` names a directory of examples built beforehand, run as
    # they are: a long run then cannot pick up an edit made while it trains,
    # which `cargo run` would rebuild into the next iteration.
    frozen = os.environ.get("STAHL_BIN_DIR")
    if frozen:
        cmd = [str(pathlib.Path(frozen).resolve() / example), *args]
        return subprocess.run(cmd, cwd=ROOT, check=True, capture_output=True, text=True).stdout
    cmd = ["cargo", "run", "--release", "-q", "-p", "tactics_core", "--example", example, "--", *args]
    return subprocess.run(cmd, cwd=ROOT, check=True, capture_output=True, text=True).stdout


EVENTS = ["contact", "hit", "friend_lost", "pinned", "order"]


def event_sets(events):
    """`balance --set` arguments turning on the goal events in `events`."""
    names = EVENTS if events == "all" else [e for e in events.split(",") if e and e != "none"]
    out = []
    for name in names:
        out += ["--set", f"planner.goal_events.{name}=true"]
    return out


def evaluate(net, places, arena, games, events="none"):
    """Learned against utility, both seatings, four seeds: (learned wins,
    battles), and the learned-against-itself row's draws and battles — the
    self-play stalemate the mission reward exists to break."""
    won = total = draws = selfs = 0
    for seed in EVAL_SEEDS:
        out = cargo("balance", "--only", "skill", "--brains", "--brain-vs", "learned",
                    "--brain-games", str(games), "--brain-difficulty", "5", "--arena", arena,
                    "--set", f"planner.goal_places={places}", *event_sets(events),
                    "--net", str(net), "--seed", str(seed))
        for line in out.splitlines():
            m = re.match(r"\s+(learned|utility) vs (learned|utility)\s+(\d+)\s+(\d+)\s+(\d+)", line)
            if not m:
                continue
            a, b, d = int(m.group(3)), int(m.group(4)), int(m.group(5))
            if m.group(1) == m.group(2):
                if m.group(1) == "learned":
                    draws += d
                    selfs += a + b + d
                continue
            won += a if m.group(1) == "learned" else b
            total += a + b + d
    return won, total, draws, selfs


FORCES = []  # set from --forces in main; passed to every rollout
APPROACH = [0]  # set from --approach in main
LAYOUT = []  # set from --spread and --turned in main: how generated ground is laid out
GROUND = ["generated"]  # set from --ground in main: generated or open, for the evaluation
COMMAND = []  # set from --commander, --opponent-commander and --budget in main


def evaluate_generated(net, places, arena, games, events="none", seed=1_000_000):
    """The same two questions on generated ground, by rollouts with the
    learner taking its best choice: against the utility AI, and against
    itself. Worlds from `seed / 2` up, which no training rollout reaches."""
    common = ["--net", str(net), "--deterministic", "--ground", GROUND[0], "--arena", arena,
              "--places", str(places), "--approach", str(APPROACH[0]), "--events", events,
              "--out", str(ROOT / "target" / "rl" / "eval.jsonl"),
              *(["--forces", FORCES[0]] if FORCES else []), *LAYOUT, *COMMAND]
    out = cargo("rollout", *common, "--games", str(games), "--seed", str(seed))
    # The learner's record with each force: overfitting to one method shows
    # as one force carrying the average.
    for line in out.splitlines():
        if "learner as" in line:
            print("    " + line.strip(), flush=True)
    m = re.search(r"against utility: won (\d+), lost (\d+), drew (\d+)", out)
    won, total = (int(m.group(1)), sum(int(g) for g in m.groups())) if m else (0, 0)
    # Against itself: every other pair of games is against the utility AI,
    # so twice the games for the same number of self-play battles.
    out = cargo("rollout", *common, "--games", str(2 * games), "--seed", str(seed + 4 * games),
                "--opponents", str(net))
    m = re.search(r"against [^:]*\.json: won (\d+), lost (\d+), drew (\d+)", out)
    draws, selfs = (int(m.group(3)), sum(int(g) for g in m.groups())) if m else (0, 0)
    return won, total, draws, selfs


CREWS = ["utility"]  # set from --crew-net in main: who chooses goals under a learned commander


def evaluate_commander(net, places, arena, games, events="none", seed=1_000_000):
    """A learned commander taking her best choice, over frozen crews, against
    no commander, the hand-written one and a coin in turn by world: her
    record against each, `(won, battles)` keyed by who commanded the other
    side, on worlds no training rollout reaches."""
    common = ["--net", CREWS[0], "--commander", "learned", "--commander-net", str(net), "--commander-deterministic", "--deterministic",
              "--ground", GROUND[0], "--arena", arena, "--places", str(places), "--approach", str(APPROACH[0]),
              "--events", events, "--opponent-commander", "mix",
              "--out", str(ROOT / "target" / "rl" / "eval_cmd.jsonl"),
              *(["--forces", FORCES[0]] if FORCES else []), *LAYOUT,
              *(["--budget", BUDGET[0]] if BUDGET else [])]
    out = cargo("rollout", *common, "--games", str(games), "--seed", str(seed))
    record = {}
    for m in re.finditer(r"  against a commander (\w+): won (\d+), lost (\d+), drew (\d+)", out):
        record[m.group(1)] = (int(m.group(2)), sum(int(g) for g in m.groups()[1:]))
    # An assault is two problems: her record attacking and defending, against
    # each commander, keyed like "attacking random".
    for m in re.finditer(r"learner (attacking|defending) against a commander (\w+): won (\d+), lost (\d+), drew (\d+)", out):
        record[f"{m.group(1)} {m.group(2)}"] = (int(m.group(3)), sum(int(g) for g in m.groups()[2:]))
    return record


BUDGET = []  # set from --budget in main


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--start", type=pathlib.Path, required=True)
    ap.add_argument("--iterations", type=int, default=40)
    ap.add_argument("--games", type=int, default=512)
    ap.add_argument("--temperature", type=float, default=1.0)
    ap.add_argument("--places", type=int, default=16)
    ap.add_argument("--approach", type=int, default=0, help="planner.approach_places: places an enemy, round his arcs")
    ap.add_argument("--arena", default="ridge_arena")
    ap.add_argument("--sizes", default="small,medium,large", help="with --ground sector, the sizes drawn per field")
    ap.add_argument("--ground", default="arena", choices=["arena", "generated", "open", "sector"],
                    help="train and evaluate on the arena, or on a fresh generated world every two games")
    ap.add_argument("--forces", help="draw matchups from these forces (a list, all, or bought)")
    ap.add_argument("--budget", help="with --forces bought, the points range LO,HI both sides buy to")
    ap.add_argument("--commander", default="none", choices=["none", "utility", "random"],
                    help="who gives the learner's platoons their missions")
    ap.add_argument("--opponent-commander", default="none", choices=["none", "utility", "random", "mix"],
                    help="who gives the opponent's platoons theirs")
    ap.add_argument("--train", default="crews", choices=["crews", "commander"],
                    help="what learns: the crews' goal network, or a commander over frozen crews (L3)")
    ap.add_argument("--crew-net", default="utility",
                    help="with --train commander, the crews' goal network, or utility for the teacher")
    ap.add_argument("--commander-temperature", type=float, default=1.0)
    ap.add_argument("--spread", type=int, default=100,
                    help="generated ground: the deployment's distance from the centre, percent of the arena's")
    ap.add_argument("--turned", action="store_true",
                    help="generated ground: turn each world's axis of advance, drawn per world")
    ap.add_argument("--events", default="none", help="planner.goal_events for every battle: a list, all, or none")
    ap.add_argument("--eval-every", type=int, default=5)
    ap.add_argument("--league", type=int, default=0, help="past checkpoints kept as opponents (0: utility only)")
    ap.add_argument("--league-every", type=int, default=5)
    ap.add_argument("--eval-games", type=int, default=36)
    ap.add_argument("--dir", type=pathlib.Path, default=ROOT / "target" / "rl" / "run")
    ap.add_argument("ppo_args", nargs="*", help="passed to ppo.py after --")
    args = ap.parse_args()
    # Absolute, because cargo runs from the repository root and ppo.py from
    # here: a relative path meant two different places.
    args.dir = args.dir.resolve()
    if args.forces:
        FORCES.append(args.forces)
    APPROACH[0] = args.approach
    LAYOUT.extend(["--spread", str(args.spread), *(["--turned"] if args.turned else []),
                   *(["--sizes", args.sizes] if args.ground == "sector" else [])])
    COMMAND.extend(["--commander", args.commander, "--opponent-commander", args.opponent_commander,
                    *(["--budget", args.budget] if args.budget else [])])
    GROUND[0] = args.ground
    args.dir.mkdir(parents=True, exist_ok=True)
    log = args.dir / "log.csv"
    if args.train == "commander":
        return train_commander(args, log)

    net = args.start.resolve()
    critic = None
    # Past checkpoints the learner also fights (`--league` of them, one added
    # every `--league-every` iterations), so it cannot learn just the one
    # opponent — and meets players as patient as itself.
    league: list[pathlib.Path] = []
    judge = evaluate if args.ground == "arena" else evaluate_generated
    games = args.eval_games if args.ground == "arena" else 8 * args.eval_games
    won, total, draws, selfs = judge(net, args.places, args.arena, games, args.events)
    print(f"start: learned {won}/{total} ({100 * won / total:.1f}%) against utility; "
          f"against itself {draws}/{selfs} drawn")
    with log.open("w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["iteration", "rollout_won", "rollout_lost", "eval_won", "eval_total",
                         "self_draws", "self_total", "seconds"])
        writer.writerow([0, "", "", won, total, draws, selfs, 0])
        for it in range(1, args.iterations + 1):
            t0 = time.time()
            rollouts = args.dir / f"rollouts{it}.jsonl"
            opponents = ["--opponents", ",".join(str(p) for p in league)] if league else []
            out = cargo("rollout", "--net", str(net), "--games", str(args.games), *opponents,
                        "--seed", str(it * args.games), "--temperature", str(args.temperature),
                        "--places", str(args.places), "--approach", str(args.approach),
                        "--arena", args.arena, "--events", args.events,
                        "--ground", args.ground, *(["--forces", args.forces] if args.forces else []), *LAYOUT,
                        *COMMAND,
                        "--out", str(rollouts), "--arrays")
            m = re.search(r"learner won (\d+), lost (\d+)", out)
            rw, rl = (int(m.group(1)), int(m.group(2))) if m else ("", "")
            new = args.dir / f"net{it}.json"
            cmd = ["uv", "run", "ppo.py", str(rollouts), "--net", str(net), "--out", str(new),
                   "--temperature", str(args.temperature), "--seed", str(it), *args.ppo_args]
            if critic:
                cmd += ["--value", str(critic)]
            ppo = subprocess.run(cmd, cwd=ROOT / "training", check=True, capture_output=True, text=True).stdout
            net, critic = new, new.with_suffix(".critic.json")
            rollouts.unlink()  # a rollout is spent once it has been learned from
            for ext in (".arrays.json", ".context.f32", ".candidates.f32", ".counts.u32"):
                pathlib.Path(str(rollouts) + ext).unlink(missing_ok=True)
            ew = et = sd = st = ""
            if it % args.eval_every == 0 or it == args.iterations:
                ew, et, sd, st = judge(net, args.places, args.arena, games, args.events)
            if args.league and it % args.league_every == 0:
                league.append(net)
                del league[: max(0, len(league) - args.league)]
            dt = time.time() - t0
            line = ppo.strip().splitlines()[-1] if ppo.strip() else ""
            ev = f"  eval {ew}/{et} ({100 * ew / et:.1f}%), self-draws {sd}/{st}" if et else ""
            print(f"iter {it:3d}  rollouts {rw}-{rl}  {line}{ev}  [{dt:.1f}s]", flush=True)
            writer.writerow([it, rw, rl, ew, et, sd, st, round(dt, 1)])
            f.flush()


def train_commander(args, log):
    """The loop for a commander (REIMAGINED.md L3): rollouts with her
    sampling her missions over frozen crews against a mix of commanders,
    one PPO update of her network on the commands stream, and every
    `--eval-every` iterations her best choices measured on fresh worlds."""
    CREWS[0] = args.crew_net if args.crew_net == "utility" else str(pathlib.Path(args.crew_net).resolve())
    if args.budget:
        BUDGET.append(args.budget)
    net, critic = args.start.resolve(), None
    games = 8 * args.eval_games

    def show(record):
        return "  ".join(f"vs {k} {100 * w / max(1, n):.1f}% of {n}" for k, (w, n) in sorted(record.items()))

    print("start: " + show(evaluate_commander(net, args.places, args.arena, games, args.events)), flush=True)
    with log.open("w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["iteration", "rollout_won", "rollout_lost", "vs_none", "vs_utility", "vs_random", "seconds"])
        for it in range(1, args.iterations + 1):
            t0 = time.time()
            rollouts = args.dir / f"rollouts{it}.jsonl"
            out = cargo("rollout", "--net", CREWS[0], "--deterministic", "--commander", "learned",
                        "--commander-net", str(net), "--commander-temperature", str(args.commander_temperature),
                        "--opponent-commander", args.opponent_commander,
                        "--games", str(args.games), "--seed", str(it * args.games),
                        "--places", str(args.places), "--approach", str(args.approach),
                        "--arena", args.arena, "--events", args.events, "--ground", args.ground,
                        *(["--forces", args.forces] if args.forces else []), *LAYOUT,
                        *(["--budget", args.budget] if args.budget else []),
                        "--out", str(rollouts), "--arrays")
            m = re.search(r"learner won (\d+), lost (\d+)", out)
            rw, rl = (int(m.group(1)), int(m.group(2))) if m else ("", "")
            new = args.dir / f"net{it}.json"
            cmd = ["uv", "run", "ppo.py", str(rollouts), "--net", str(net), "--out", str(new), "--stream", "commands",
                   "--temperature", str(args.commander_temperature), "--seed", str(it), *args.ppo_args]
            if critic:
                cmd += ["--value", str(critic)]
            ppo = subprocess.run(cmd, cwd=ROOT / "training", check=True, capture_output=True, text=True).stdout
            net, critic = new, new.with_suffix(".critic.json")
            rollouts.unlink()
            for stream in ("", ".cmd"):
                for ext in (".arrays.json", ".context.f32", ".candidates.f32", ".counts.u32"):
                    pathlib.Path(str(rollouts) + stream + ext).unlink(missing_ok=True)
            record = {}
            if it % args.eval_every == 0 or it == args.iterations:
                record = evaluate_commander(net, args.places, args.arena, games, args.events)
            dt = time.time() - t0
            line = ppo.strip().splitlines()[-1] if ppo.strip() else ""
            ev = ("  eval " + show(record)) if record else ""
            print(f"iter {it:3d}  rollouts {rw}-{rl}  {line}{ev}  [{dt:.1f}s]", flush=True)
            pct = lambda k: round(100 * record[k][0] / max(1, record[k][1]), 1) if k in record else ""
            writer.writerow([it, rw, rl, pct("none"), pct("utility"), pct("random"), round(dt, 1)])
            f.flush()


if __name__ == "__main__":
    main()
