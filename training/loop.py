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
import pathlib
import re
import subprocess
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
EVAL_SEEDS = [100000, 101000, 102000, 103000]


def cargo(example, *args):
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


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--start", type=pathlib.Path, required=True)
    ap.add_argument("--iterations", type=int, default=40)
    ap.add_argument("--games", type=int, default=512)
    ap.add_argument("--temperature", type=float, default=1.0)
    ap.add_argument("--places", type=int, default=16)
    ap.add_argument("--arena", default="ridge_arena")
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
    args.dir.mkdir(parents=True, exist_ok=True)
    log = args.dir / "log.csv"

    net = args.start.resolve()
    critic = None
    # Past checkpoints the learner also fights (`--league` of them, one added
    # every `--league-every` iterations), so it cannot learn just the one
    # opponent — and meets players as patient as itself.
    league: list[pathlib.Path] = []
    won, total, draws, selfs = evaluate(net, args.places, args.arena, args.eval_games, args.events)
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
                        "--places", str(args.places), "--arena", args.arena, "--events", args.events,
                        "--out", str(rollouts))
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
            ew = et = sd = st = ""
            if it % args.eval_every == 0 or it == args.iterations:
                ew, et, sd, st = evaluate(net, args.places, args.arena, args.eval_games, args.events)
            if args.league and it % args.league_every == 0:
                league.append(net)
                del league[: max(0, len(league) - args.league)]
            dt = time.time() - t0
            line = ppo.strip().splitlines()[-1] if ppo.strip() else ""
            ev = f"  eval {ew}/{et} ({100 * ew / et:.1f}%), self-draws {sd}/{st}" if et else ""
            print(f"iter {it:3d}  rollouts {rw}-{rl}  {line}{ev}  [{dt:.1f}s]", flush=True)
            writer.writerow([it, rw, rl, ew, et, sd, st, round(dt, 1)])
            f.flush()


if __name__ == "__main__":
    main()
